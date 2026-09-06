---
id: FEAT-016
title: Project Environment Recipes - a Project Tells Every Machine What It Needs
status: Accepted
tier: 2
date: 2026-09-06
delivered:
priority: High
areas: [runner, agents, dependencies, security]
tags: [feature, runner, executors, environment, tools, onboarding, dependencies, security, agent-first]
---

# Feature: Project Environment Recipes - a Project Tells Every Machine What It Needs

## Summary

Every onboarded project carries one file, `docs/context/environment.md`,
whose frontmatter says which tools (exact versions) its checks need, which
setup lines make a fresh clone runnable, which non-secret variables to
export, and which check it suggests the human set as `verify_cmd`. The onboarding agent writes
the first version from what it observed; later agents improve it. The runner
reads the recipe of the base commit it clones, installs the pinned tools
user-locally through `mise` on whichever machine takes the job, runs the
setup before the worker and every check, and reports fitness per machine in
`pulse env check`, `pulse status`, and a read-only dashboard card. A recipe
change travels on a branch of its own and lands only after the human - by
hand at first, through `pulse decide ... yes` once the proposal path exists
- until the human sets `env_landing agents`. Decision:
[ADR-026](../decisions/ADR-026-project-environment-recipes.md); plan:
[PLAN-026](../plans/PLAN-026-project-environment-recipes.md).

## User Story

As the maintainer, I want each project I onboard to tell the machines what
it needs - and the agents to write and improve that description - so that a
new machine or a new project runs green without me installing things by
hand, while nothing runs on my machines that I have not said yes to.

As an agent, I want one place to state a project's environment and one
command that shows which machines can run it, so that a job fails for a
reason I can fix rather than for a tool nobody wrote down.

## Requirements

### Functional

- [ ] `parse_recipe` accepts exactly `recipe`, `tools`, `setup`, `env`,
      `services`, `suggested_check`; refuses unknown keys, `recipe != 1`,
      non-pinned tools, pins outside the argv pattern, more than 12 tools,
      non-string setup lines, env names outside `^[A-Z_][A-Z0-9_]*$`,
      multi-line env values, secret-shaped env names and values (including
      `://user:pass@` URLs and `eyJ` JWTs); error messages name the key and
      never include an env value.
- [ ] The runner reads the recipe at the **base commit** (`git show
      <base>:docs/context/environment.md`); the branch's own recipe never
      configures its own check (recipe path in `CHECK_FILES`).
- [ ] `setup:` replaces the WU-021 lockfile heuristic when present; an
      absent recipe or key keeps the heuristic.
- [ ] `env:` is exported for setup, the worker, and every check on both
      executor kinds; the hands accept those keys only when named in
      `PULSE_RECIPE_ENV`, refuse secret-shaped names and duplicates, and
      pass values with spaces intact.
- [ ] `tools:` are installed with `mise install name@version` when missing
      and every command runs through `mise exec name@version ... --` built
      as argv on both sides of the ssh line, never by string expansion;
      `.tool-versions` and `mise.toml` in the tree are not read.
- [ ] An executor whose hands predate the feature is unfit `hands outdated`
      for any recipe with `tools:` or `env:` before a job is placed there;
      `setup:`-only recipes still run there through `verify`.
- [ ] `mise` missing, a tool that fails to install, or a non-empty
      `services:` marks the (executor, project) pair unfit with a reason;
      the job is requeued, the attempt returned, the executor not drained;
      the `tools-wanted` finding names the human's exact line.
- [ ] `pulse onboard` scaffolds the recipe skeleton only when absent, never
      stages it; the onboarding brief asks for the filled recipe.
- [ ] `pulse env check [--project P] [--executor E] [--json]` is read-only,
      lists tools by executor, shows `suggested_check:` beside `verify_cmd`,
      exits
      non-zero when any executor is unfit; the daemon runs it daily per
      project with `actor="daemon"` (never clearing marks).
- [ ] A branch that changes the recipe and any other file is refused at
      landing (`result`: *recipe changes travel alone*).
- [ ] A recipe-only branch (slice 1): held, one `question` posted, landed
      only by the human's `pulse land` or hand merge.
- [ ] A recipe-only branch (slice 3): landing check under the base recipe
      (advisory) -> validate the branch's recipe, dry-run its setup, run the
      check under the proposed recipe, post one `answer` (`resolution:
      agreed`) on `runner/environment` with both outcomes, leave the branch;
      `pulse decide <id> yes` -> the daemon lands it on the next loop; `no`
      -> branch removed, `result` says `declined`; `env_landing agents` ->
      lands like any other file.
- [ ] A setup failure on a recently landed recipe sets `env_landing` back to
      `human`, logs `env.incident`, posts a `finding`.
- [ ] `pulse status` and the evolution report carry the measurements of
      ADR-026 rule 7; the dashboard shows an Environment card, read-only.

### Non-Functional

- [ ] **Performance:** tool installs and setup share the existing 10-minute
      setup cap; a warm executor (tools already present) adds under two
      seconds for `mise exec` resolution.
- [ ] **Security:** recipe commands run as the executor user with no
      `sudo`; installs are user-local and exactly pinned; secrets are
      refused in the recipe and in the hands; no new dashboard write route;
      no store schema change; a recipe never changes `verify_cmd`.
- [ ] **Reliability:** an invalid recipe never reaches the worker; unfit
      marks expire after 24 hours and are cleared by a human probe; every
      failure names its reason in the executors line.

## Scope

### Area

- [ ] `docs/scripts/pulse.py` - parser, runner integration, `env check`,
      landing category, setting, measurement.
- [ ] `docs/scripts/hosted/pulse-hands.sh`, `cloud-init.yaml` - `tools`
      verb, `run` env allowance, `mise exec` wrap, `mise` install.
- [ ] `docs/context/_template-environment.md`, `docs/context/README.md`.
- [ ] `docs/scripts/pulse-dashboard.py`, `docs/hub/` - Environment card,
      settings whitelist row.
- [ ] Tests and workflow docs named in PLAN-026.

## Interface Changes

- New file per project: `docs/context/environment.md` (format in ADR-026
  rule 1; keys `recipe`, `tools`, `setup`, `env`, `services`,
  `suggested_check`).
- CLI: `pulse env check [--project P] [--executor E] [--json]` (read-only);
  `pulse settings set env_landing human|agents`; `pulse onboard` scaffolds
  the recipe; `pulse status` executors line reads `lacks mise`, `lacks
  <tool>`, `needs services`, `hands outdated`; `pulse proposals` lists
  recipe proposals; `pulse land --project P --branch B` is the human's way
  to land a held recipe branch before the proposal path exists.
- Hands verbs: `probe` reports `verbs=`; `tools NAME...` (new, read-only);
  `run` accepts `PULSE_RECIPE_ENV`-listed keys and wraps in a `mise exec`
  array when `PULSE_TOOLS` is set. Machines paired before this feature are
  unfit for `tools:`/`env:` recipes until `pulse pair` is re-run.
- Board: `finding` on `runner/environment` (invalid recipe, incident);
  `answer` proposals on `runner/environment`; `result` `declined`.
- Dashboard: Environment card per project; `env_landing` in Settings.

## Data Changes

- No schema change. New settings row `env_landing`. New `unfit` reasons in
  the `meta` row (`mise`, `tool:<name>`, `services`, `hands outdated`).
  New event kinds `env.held`,
  `env.proposed`, `env.landed`, `env.declined`, `env.incident`. Recipe
  proposals are ordinary `answer` posts.

## Decisions

- [ADR-026](../decisions/ADR-026-project-environment-recipes.md) - this
  feature.
- [ADR-024](../decisions/ADR-024-agent-driven-landing.md) - landing by
  agents; the human-landed category is its narrow addition.
- [ADR-021](../decisions/ADR-021-top-down-onboarding.md) - `pulse onboard`
  and `pulse pair`, which this feature extends.
- [ADR-020](../decisions/ADR-020-remote-executors.md) - the hands' forced
  command posture, unchanged.
- [WU-021](../plans/WU-021-runnable-clones.md) - the heuristic that stays
  as fallback and the unfit mechanism this feature reuses.

## Edge Cases

- Recipe absent at the base commit but present on the branch (the first
  recipe): heuristic runs the check; the branch is proposed for landing;
  the next jobs use the recipe.
- Recipe present, `setup: []`: no setup runs at all (explicit empty beats
  the heuristic); the brief says so.
- `tools:` non-empty and `mise` missing on the local executor: unfit with
  reason `mise`, job moves to a fit executor or waits; `pulse env check`
  prints the install line for the human.
- Two projects pin different `node` versions on one machine: both installed
  under `~/.local/share/mise`; each command sees only its own pin.
- A tool name mise's registry does not know: install fails -> unfit
  `tool:<name>` for 24 hours; the finding names it; an agent may propose a
  recipe fix (human-landed).
- `env:` contains `DATABASE_URL: postgres://user:pass@host/db`: value
  matches the secret pattern (`pass@` in a URL is treated as a credential)
  -> recipe invalid; message names `DATABASE_URL`, not the value.
- `env:` contains `API_KEY_NAME: "x"`: the name matches `*_KEY*` -> invalid
  even though the value is harmless (names are the rule; a project renames
  it).
- Recipe changed together with product code on one branch: refused at
  landing, branch removed, `result` says *recipe changes travel alone*; the
  agent re-proposes the recipe on its own branch.
- First recipe for a project the heuristic cannot make runnable (the check
  fails under the base): the proposal still reaches the human, with *check
  under the base recipe: fail* and *setup and check under the proposed
  recipe: pass* both stated; the human decides with that in view.
- A `tools:` pin like `node@22; rm -rf ~` or a name with a space: refused
  by the argv pattern at parse time on the hub and again by the hands; no
  shell ever sees it as words.
- Env value with a space (`CFLAGS: "-O2 -g"`): allowed; reaches setup,
  worker, and check intact on both executor kinds (tested).
- Two branches change the recipe: two proposals; the second one's landing
  re-runs the check on the new base after the first lands (existing landing
  rules on stale branches apply).
- `pulse decide yes` while the branch was removed by hand: `land_pending`
  records `missing`, the `result` says so, nothing else changes.
- `env_landing agents` and a landed recipe breaks setup on the next job:
  incident -> setting back to `human`, finding posted, clean count 0.
- Worker edits `.tool-versions` or `mise.toml` to change the check's
  runtime: ignored by design (not read); the recipe's pins rule.
- Remote hands older than this feature (probe without `verbs=`): the
  executor is unfit `hands outdated` for recipes with `tools:` or `env:`
  before any job is placed; `setup:`-only recipes still run there;
  `env check` reads `hands outdated - re-run pulse pair`.
- `services: [postgres@16]`: unfit everywhere with reason `services`; the
  onboarding agent's recipe still records it so the follow-up decision has
  the facts.
- Recipe frontmatter valid, prose contains a secret: not parsed, not
  executed; the secret rule covers only what the runner reads. The review
  gate (human-landed) is the defence, and the memory entry says so.

## Rollback Strategy

- **Baseline:** brain `main` at each slice's landing commit; the live
  store's settings rows; `pulse-vm1`'s hands source hash.
- **Trigger:** suite red after a slice; a recipe-less project's job regresses
  on setup; the hands refuse a previously accepted `run`; a recipe lands
  without a recorded yes under `env_landing human`.
- **Reversal:** `git revert` the slice commit(s); re-run `pulse pair` for the
  previous hands; `pulse settings set env_landing human`; adopter recipe
  files stay (ignored after the revert).
- **State safety:** no schema change; user-local tools are inert; branches
  waiting on proposals are ordinary job branches.
- **Recovery verification:** full suite; `py_compile`; `bash -n`; one
  heuristic job green; `pulse executor probe` `hands=ok`.

## Generated Prompts

- None yet. PLAN-026's slices are handed to agents as `improve` jobs with
  the slice text as the brief once ADR-026 is accepted; a shared prompt is
  written only if the first slice shows the handoff needs one.

## Acceptance Criteria

- [ ] Every functional requirement above has a test in
      `docs/scripts/tests/test_pulse.py` and passes.
- [ ] Every Edge Cases line is walked against the code before closure.
- [ ] `project-light`'s second job after its recipe lands runs green on
      `pulse-vm1` with the human having installed only `make`.
- [ ] Docs in PLAN-026 updated; `delivered:` set here and in the plan;
      ADR-026 `delivery:` updated.

## Verification Plan

- `python3 -m unittest docs/scripts/tests/test_pulse.py`
- `python3 -m py_compile docs/scripts/pulse.py && bash -n docs/scripts/hosted/pulse-hands.sh && git diff --check`
- Live: `pulse env check --project project-light`; `pulse status`;
  `pulse proposals`; one job on the paired machine after `pulse pair` is
  re-run by the human.
