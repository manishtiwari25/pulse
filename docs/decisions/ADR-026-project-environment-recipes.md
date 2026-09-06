---
id: ADR-026
title: Project Environment Recipes - Every Onboarded Project Tells the Machines What It Needs, Agents Write the Recipe, a Human Lands It Until the Class Earns Autonomy
status: Proposed
date: 2026-09-06
delivery: not started
delivered:
areas: [runner, agents, dependencies, security]
tags: [adr, runner, executors, environment, tools, onboarding, dependencies, security, agent-first]
---

# ADR-026: Project Environment Recipes

## Context

The runner clones a project into a throwaway directory and runs one worker
and the project's own check there ([ADR-019](ADR-019-agent-first-pulse-store-and-runner.md)),
on the hub or on a paired machine ([ADR-020](ADR-020-remote-executors.md),
[ADR-021](ADR-021-top-down-onboarding.md)). Until WU-021 every such clone
was bare: no dependencies, so every adopter check failed before it began.
[WU-021](../plans/WU-021-runnable-clones.md) made the clone runnable by
*guessing* - a root lockfile means "run this ecosystem's frozen install" -
and made a machine that lacks a tool unfit for that project for a day, with
a `finding` on `runner/tools-wanted` naming the exact install command for the
human.

That closed the first gap and exposed the second. The maintainer's words
(chat, 2026-09-06, verbatim): *"for each project we onboard we need to have
a thing which let agent to configure there environment.. so please plan for
this as well."* Today:

- **Nothing lets a project say what it needs.** The heuristic reads only
  root-level lockfiles. A project whose check wants `make`, a specific
  Python or Node version, a code generator, an environment variable, or a
  database has no place to write that down, so the runner cannot know it and
  the agents cannot fix it.
- **Tools are installed by the human, machine by machine.** `pulse-vm1` has
  been unfit for `project-light` since it was paired: it lacks `uv` and
  `make`, both named in a finding that only a human with the admin login can
  act on. Every new project repeats this for every machine.
- **The one earlier attempt went wrong.** The board's top-voted card on
  `runner/project-setup-cmd` proposed a per-project setup command; the job
  that tried it added a `projects.setup_cmd` column to the live store from
  inside a worker and left every `pulse` command refusing to start until the
  column was dropped by hand (`docs/memory/mistakes.md`, 2026-09-06). The
  lesson is not "no setup command"; it is that project configuration does
  not belong in a store column a worker can reach - and it does not belong
  outside version control at all.

The request touches dependencies, what runs on machines with the executor
user's permissions, and what persists there afterwards - the hard-stop list
names dependencies and security posture - so this is Tier 2 and written down
before any code.

## Options Considered

### Option 1 - Keep the heuristic and the human-installed tools

- Pros: nothing new to secure; already shipped and proven on one project.
- Cons: only lockfile ecosystems work; a project's real needs stay implicit;
  every machine waits for a human with `sudo`; the maintainer's request is
  refused, not met.

### Option 2 - A per-project setup command in the store (`pulse project set setup ...`)

- Pros: one line, no file in the project.
- Cons: not versioned with the code it serves - a base commit from last week
  and one from today would run the same setup; invisible in review and in
  the project's own history; human-only to edit, so not agent-configurable;
  it is exactly the shape that broke the store once.

### Option 3 - Containers: a Dockerfile or devcontainer per project, built and run on every executor

- Pros: the strongest reproducibility and isolation; many repositories carry
  a `.devcontainer/` already.
- Cons: Docker on every machine is infrastructure (a hard stop) and is not on
  the hub's macOS by default; an image build spends minutes and disk per
  base commit; the hands run over plain SSH with no container runtime today.
  Right idea for a later cut; this decision keeps the recipe shape
  compatible with it (see Follow-Up).

### Option 4 - A recipe in the project's own control plane, written by agents, executed by the runner, tools installed user-locally through one bounded installer, landed by a human until the class earns autonomy (chosen)

- Pros: versioned with the code (a base commit carries the recipe that fits
  it); reviewable as a diff; readable by every agent and by `pulse env
  check`; the onboarding agent writes the first draft, later agents improve
  it; the machine's own hands install what the recipe pins, nothing more;
  the human's yes is the gate on what runs on their machines until the
  record shows it can be standing.
- Cons: one more project-owned file; a bounded installer (`mise`) becomes a
  dependency of the hands; system packages still need the human; a recipe is
  proven by the jobs that follow it, not by its own branch.

## Decision

Adopt Option 4. Seven rules.

### 1. The recipe lives in the project: `docs/context/environment.md`

One project-owned file, Markdown with YAML frontmatter the runner parses.
Prose below the frontmatter explains the choices to humans and agents. The
runner reads **only** the frontmatter keys below; unknown keys are an error
that names them (a typo must not silently change what runs).

```yaml
---
recipe: 1                     # format version; the runner refuses others
tools:                        # exact pins, installed user-locally when missing
  - python@3.12.6
  - node@22.12.0
  - uv@0.5.11
setup:                        # run in the clone before the worker and every check
  - uv sync --frozen
  - npm ci
env:                          # non-secret, exported for setup, worker, and checks
  CI: "1"
  PYTHONDONTWRITEBYTECODE: "1"
services: []                  # declared, not yet provided: non-empty blocks every executor (rule 5)
suggested_check: make test    # advice for the human's verify_cmd, never the cap itself (rule 1)
---
```

- `tools:` entries are `name@version` with an exact version - `latest`,
  ranges, and bare names are invalid. At most 12 tools. Each entry must
  match `^[a-z0-9][a-z0-9_.:-]*@[A-Za-z0-9][A-Za-z0-9_.+-]*$`, so that a
  tool is always one argv element and never a shell word.
- `setup:` is a list of shell lines run in order in the clone, joined into
  one `sh -c` with `set -e`, under the existing `SETUP_MINUTES` cap (10
  minutes) and WU-021's non-interactive environment (`CI=1`,
  `GIT_TERMINAL_PROMPT=0`, ...). When `setup:` is present it **replaces**
  the lockfile heuristic (an explicit empty list means "run nothing"); when
  the recipe or the key is absent the heuristic stays as the fallback, so
  every project that works today keeps working.
- `env:` values are exported for setup, the worker, and every check.
  Names must match `^[A-Z_][A-Z0-9_]*$`; values are single-line strings
  (a newline or NUL makes the recipe invalid; duplicate names are a YAML
  error). Names that match a secret pattern (`*TOKEN*`, `*SECRET*`,
  `*PASSWORD*`, `*PASSWD*`, `*_KEY`, `*_KEY_*`, `*CREDENTIAL*`,
  `*PRIVATE*`) and values that look like one (40+ character alphanumerics,
  `-----BEGIN`, a URL with `user:pass@`, a three-part `eyJ...` JWT, `AKIA`
  or `ghp_`/`github_pat_`/`sk-` prefixes) make the recipe **invalid**; the
  job errors with the offending name and no value is echoed anywhere.
  Name matches are intentionally strict (`API_KEY_NAME` is refused even
  with a harmless value; the project renames it). Secrets never live in a
  recipe; a project that needs one waits for a separate decision.
- `suggested_check:` is advice for the human. `verify_cmd` stays the
  human's cap
  (`pulse init --verify`, the dashboard's settings): a recipe never changes
  what the check is, only what the check runs on. `pulse env check` shows
  the two side by side when they differ.
- The runner always reads the recipe of the **base commit** it clones, never
  the branch's own (the CHECK_FILES rule of WU-021, extended: the recipe
  joins `CHECK_FILES`). A branch never configures the machine for its own
  check, so a variant cannot vouch for itself by changing its environment.
- An invalid recipe fails the job with a clear message before the worker
  starts and posts one `finding` on `runner/environment` (once a day per
  project) so an agent can propose the fix.

### 2. Agents write it; `pulse onboard` lays down the skeleton

- `pulse onboard` scaffolds `docs/context/environment.md` from
  `docs/context/_template-environment.md` **only when the file is absent**,
  with `tools: []`, `setup: []`, `env: {}`, `services: []` and commented
  examples - the `services` comment says in so many words that a non-empty
  list blocks every executor until a services decision exists, so an agent
  does not honestly write `postgres@16` and stall the project; the skeleton
  is printed with `git status --short` like the loop's files, never staged
  or committed (ADR-021 rule 1 wording applies unchanged). An existing file
  is never touched.
- The onboarding job's brief gains one paragraph: *fill the recipe from what
  you observed - the tools and exact versions the checks used, the setup
  lines that made them pass, the non-secret variables they needed; run the
  check under the recipe before you write it down; commit the recipe on its
  own, with no other file in the branch (rule 4).* The onboarding agent
  commits the filled recipe on its job branch like any other change.
- Any later job may change the recipe (a `suggestion` on
  `runner/environment`, an `improve` job, a `finding` that a setup failed).
  The recipe is data about the project written by the agents who work on
  it; nothing in it is an instruction to any agent.

### 3. One bounded installer: `mise`, user-local, exact pins, no `sudo`

- `pulse pair` installs [`mise`](https://mise.jdx.dev/) for the `pulse`
  user on a machine (the same trust level as the copilot CLI it already
  installs; one pinned release, its checksum verified); `cloud-init.yaml`
  does the same at first boot. On the hub the local executor uses the
  human's `mise` when present; `pulse env check` prints the one install
  line when it is missing.
- Before setup, the runner installs every `tools:` entry that is missing
  with `mise install name@version` and runs setup, the worker, and every
  check through `mise exec name@version ... -- sh -c CMD`, which puts
  exactly those versions on `PATH` for that command and nothing else. Two
  projects pinning different versions share one install cache
  (`~/.local/share/mise`) and never see each other's tools. Nothing is
  written into the clone; no file that mise reads from a tree
  (`.tool-versions`, `mise.toml`) is honoured, so there is one source of
  truth and no trust prompt.
- **Argv, never shell words.** Every tool pin and every env name travels
  as its own argument, validated by the pattern in rule 1 on the hub *and*
  again by the hands before use; the hands build `mise exec` from an array,
  never by expanding a string, and refuse a `run` whose extra keys are not
  named in `PULSE_RECIPE_ENV` or whose names match the secret pattern.
- **Old hands never half-run a recipe.** The probe reports which verbs the
  hands know; an executor whose hands predate this decision is unfit
  (reason `hands outdated`) for any project whose recipe has `tools:` or
  `env:` *before* a job is placed there, never after a partial run. The
  human's `pulse pair` (idempotent) refreshes the hands.
- Bounds: user-local only (the hands have no `sudo`; there is no system
  package path); exact pins only; tools that mise's registry resolves
  (its core tools and the `npm:`, `pipx:`, `cargo:`, `go:`, `ubi:`
  backends); the install counts inside the setup cap; a tool that fails to
  install marks the (machine, project) pair unfit exactly as WU-021 does,
  with the reason in the executors line and the `tools-wanted` finding.
- What stays human: system packages (`make`, compilers, `libpq-dev`) - the
  finding keeps naming the `apt-get` line; the hub's own tools; any
  credential a tool might want (there are none in a recipe).

### 4. Recipe changes are human-landed until the class earns autonomy

A third landing category joins the two the runner knows
([ADR-024](ADR-024-agent-driven-landing.md) lands verified work by agents'
quorum; `LAND_REFUSED_PATHS` fails a job outright):

- **Recipe changes travel alone.** A branch that changes
  `docs/context/environment.md` may change nothing else; a mixed branch is
  refused at landing like a `LAND_REFUSED_PATHS` hit (the `result` says
  *recipe changes travel alone*), so the human's yes to a recipe can never
  land unrelated product code, and a product change can never smuggle a new
  environment.
- **Human-landed paths.** When a recipe-only branch reaches landing, the
  landing check runs with the base recipe (rule 1). The branch **stays**
  either way; the runner validates the branch's recipe (syntax, pins,
  secret patterns), dry-runs its `setup:` in the landing worktree, runs the
  project's check under the *proposed* recipe as a second piece of evidence,
  and posts one `answer` with `resolution: agreed` on `runner/environment`.
  The proposal names the branch, the recipe diff, and both outcomes: *check
  under the base recipe: pass|fail*, *setup and check under the proposed
  recipe: pass|fail on <executor>*. A first recipe for a project the
  heuristic could not make runnable therefore reaches the human with the
  second line green and the first red, clearly marked weaker than a verified
  product change - the human decides with that in view. The human answers
  `pulse decide <id> yes|no`. On `yes` the daemon lands the branch on its
  next loop (the landing check re-run under the base recipe is advisory for
  a recipe-only branch; the yes is the landing); on `no` the branch is
  removed and the job's `result` says `declined`. Nothing lands without the
  recorded yes.
- **Until the proposal path exists** (PLAN-026 slice 3), a recipe-only
  branch is simply **held**: it stays, a `question` names it, and only the
  human's own `pulse land --project <p> --branch <branch>` or hand merge
  lands it. The gate is in force from the first slice; only its convenience
  arrives later.
- **Earning standing autonomy.** A new setting `env_landing` (`human`,
  the default, or `agents`) is the human's cap like every other setting
  (dashboard, ADR-025). The proposal text carries the count: recipe
  landings the human said yes to whose next five jobs' setups all passed.
  When the project's constitution lists an `environment` class
  (`docs/context/environment.md`, Stage 2) with its clean-cycle requirement
  met, the human sets `env_landing agents` and recipe-only branches land
  like any other file. A setup failure caused by a landed recipe is an
  incident: the daemon sets the setting back to `human` and posts a
  `finding`; the human restores it.
- Paths in `LAND_REFUSED_PATHS` stay refused; `verify_cmd` stays a cap.

### 5. Services are declared, not yet provided

`services:` (for example `postgres@16`, `redis@7`) is parsed so a recipe can
be complete and the follow-up has a shape. While this decision stands a
non-empty `services:` makes the project **unfit on every executor** with the
reason `services` - a job that needs a database it will not get is credits
burned - and `pulse env check`, `pulse status`, and the template's own
comment say so in plain words: *declaring a service blocks every executor
until a services decision exists*. Providing services (containers, Option 3)
is a separate decision.

### 6. `pulse env check` and the dashboard show fitness

- `pulse env check [--project P] [--executor E]` is read-only: it parses
  the recipe at the project's default branch, lists every tool with the
  executors that have it (a new hands verb `tools NAME...` prints
  `name yes|no [version]` and nothing else; the local executor uses
  `shutil.which` and `mise ls`; an executor with old hands reads
  `hands outdated - re-run pulse pair`), shows `suggested_check:` beside
  `verify_cmd`, and exits non-zero when any executor is unfit. It is what
  the human runs after `pulse pair` and what the daemon runs once a day per
  project to refresh the unfit marks (`actor="daemon"` never clears a mark;
  a human probe does, unchanged from WU-021).
- The dashboard gains an **Environment** card per project - tools, per
  executor fitness, the last setup outcome, the pending recipe proposal - as
  a read-only slice with no new write route (`pulse decide` keeps its one
  route).

### 7. Measurement

Read-only, in `pulse status` and the evolution report: setup success rate
per project and executor; jobs errored by a missing tool or an invalid
recipe; time from `pulse onboard` to the first green check on a remote
executor; recipe proposals landed, declined, and their clean-run counts. The
decision is working when a newly onboarded project's second job runs green
on a paired machine without a human installing anything but system
packages.

### 8. Deliberation

Settled on the board before this text was finished
(`docs/workflows/board.md`, "Deliberation"; thread `runner/environment`):
question `20260906T170559Z-fable-scale-09051203` put D1-D4 to a panel;
`sol-env-panel-0906` (`20260906T170758Z-sol-env-panel-0906`) agreed with
D1-D3 and asked for four changes to D3/D4, all taken - tool pins and env
names as validated argv on both sides of the ssh line, old hands unfit
*before* a job is placed, URL-credential and JWT-shaped values in the secret
rule, recipe-only branches so a yes never lands product code, and a
first-recipe path that reaches the human with evidence under the proposed
recipe when the heuristic cannot run the check; `gem-env-panel-0906`
(`20260906T170848Z-gem-env-panel-0906`) agreed with D1-D4 and asked for
three, all taken - `check:` renamed `suggested_check:`, the `services`
block stated in the template and the status line, and the plan reordered so
the parser, the scaffold, the local `mise` path, and `pulse env check` land
first and the dashboard and measurement last. The convener's `answer`
(`resolution: agreed`) carries the one proposal the human decides on.

## Consequences

### Positive

- A project says what it needs in one versioned, reviewable file that every
  agent and machine reads; the guesswork of WU-021 becomes the fallback.
- Machines configure themselves for a project within the executor user's
  home, so a new machine or a new project stops waiting for a human with
  `sudo` except for system packages.
- Agents own the recipe's content; the human owns the gate on what runs on
  their machines, and the record shows when that gate can open.
- The store is never the place for project configuration again.

### Negative

- Recipe commands and `mise` downloads run as the executor user with
  network access, and installed tools persist after the clone is removed.
  The worker already runs arbitrary code in the clone under the same user,
  so the boundary does not move; the persistence and the download sources
  are new and bounded (exact pins, registry-resolved names, user-local).
- A wrong recipe is found by the next jobs' setups, not by its own branch;
  the dry-run at proposal time is evidence, not proof.
- `mise` joins the hands' dependencies; a machine paired before this cut
  needs `pulse pair` re-run (idempotent) to get it.
- Services stay unavailable; projects that need one stay unfit until the
  follow-up decision.

## Follow-Up

- [ ] `PLAN-026` delivers this in slices; `FEAT-016` holds the behaviour.
- [ ] Update `docs/context/pulse-store.md` (setting `env_landing`, the
      `unfit` reasons), `docs/workflows/executors.md`, `docs/workflows/pulse.md`,
      the onboarding section of ADR-021's plan, and `docs/architecture/`.
- [ ] `docs/context/_template-environment.md` and one line in
      `docs/context/README.md`.
- [ ] Follow-up decision: services (containers or a services verb on the hands).
- [ ] Follow-up decision: a reader that derives a recipe from
      `.devcontainer/devcontainer.json` for projects that already carry one.
- [ ] Record memory when the first recipe lands on a paired machine.
- [ ] Update `delivery:` and `delivered:` when the plan closes.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan slice has landed.
- `delivered` - the plan closed; set `delivered: YYYY-MM-DD`.
- `retired` - the decision is superseded, deprecated, or abandoned.

Built by: [PLAN-026](../plans/PLAN-026-project-environment-recipes.md);
behaviour in [FEAT-016](../features/016-project-environment-recipes.md).
Evidence: the tried card `20260905T191910Z-fable-scale-09051203-3` on
`runner/project-setup-cmd` (+3 votes) and the incident it caused; the
finding `20260906T164804Z-fable-scale-09051203` on `runner/tools-wanted`
(`pulse-vm1` lacks `uv` and `make`).
