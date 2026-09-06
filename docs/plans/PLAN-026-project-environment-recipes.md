---
id: PLAN-026
title: Project Environment Recipes - Deliver ADR-026 in Three Slices
status: Draft
tier: 2
date: 2026-09-06
delivered:
tags: [plan, runner, executors, environment, tools, onboarding, dependencies, security]
---

# Plan: Project Environment Recipes

## Tier

Tier 2 - a project-owned recipe that runs on machines with the executor
user's permissions, installs tools that persist there, and changes how the
runner lands one class of file: dependencies and security posture, which no
ADR covered. Decision:
[ADR-026](../decisions/ADR-026-project-environment-recipes.md). Behaviour:
[FEAT-016](../features/016-project-environment-recipes.md). Nothing below
starts before the human's `pulse decide ... yes` on the deliberation's
agreed answer (thread `runner/environment`).

## Goal

A newly onboarded project's second job runs green on a paired machine
without a human installing anything but system packages: the onboarding
agent writes `docs/context/environment.md`, the human lands it once, and
from then on every executor installs the pinned tools user-locally, runs the
recipe's setup before the worker and every check, and reports its fitness in
`pulse env check`, `pulse status`, and the dashboard. WU-021's lockfile
heuristic remains the fallback for projects without a recipe.

## Context

- Runner: `docs/scripts/pulse.py` - WU-021's `SETUP_RULES`,
  `setup_command(root, rev)`, `missing_tool`, `check_tool_missing`,
  `unfit_marks/mark_unfit/unfit_projects/clear_unfit` (meta key `unfit`),
  `tools_wanted`, `ExecutorUnfit`; `run_job` (setup after `ex.prepare()`,
  check-cannot-start block, finalization); `write_brief(..., setup=)`;
  `take_job` unfit filtering; `probe_executor` (`actor == "human"` clears
  marks); `land_branch` (setup in the landing worktree; `deferred` on setup
  failure); `land_pending` (only when `landing == agents`);
  `LAND_REFUSED_PATHS`, `CHECK_FILES`, `refused_paths`, `branch_files`;
  `queue_onboarding_job` (the brief); `cmd_onboard` (scaffold of
  `docs/evolution/` when entirely absent); `pair_apply_script`,
  `pair_inspect_script` (hands hash, copilot install); `SETTING_RULES` and
  the defaults dict (`landing`); `decide()`; `executors_line`;
  `settings_report`.
- Hands: `docs/scripts/hosted/pulse-hands.sh` - verbs `probe`, `prepare`,
  `key`, `brief`, `run JOB MINUTES [K=V ...] -- CMD` (accepts only `PULSE_*`
  and `GIT_TERMINAL_PROMPT` keys and only the copilot command), `verify JOB
  MINUTES -- CMD` (a shell string), `clean`; the `PATH` prepend of
  `~/.local/bin`, `~/.cargo/bin`, `~/.bun/bin` (WU-021). `cloud-init.yaml`
  installs git, python3, Node.js 22, the copilot CLI.
- Dashboard: `docs/scripts/pulse-dashboard.py` and `docs/hub/` (ADR-023
  stack); the settings whitelist (ADR-025) is the only write path besides
  `decide`.
- Tests: `docs/scripts/tests/test_pulse.py` (`RunnableCloneTestCase`,
  `RunnableCloneRemoteTestCase` with the ssh shim that logs base64 argv;
  `MigrationTestCase`; landing tests). Suite: 259 tests, 1 skip.
- Docs to update: `docs/workflows/executors.md` ("What a machine needs
  installed"), `docs/workflows/pulse.md`, `docs/context/pulse-store.md`
  (settings, `unfit` reasons), `docs/context/README.md`, `AGENTS.md` (one
  line under the runner paragraph), `docs/plans/README.md`,
  `docs/features/README.md`, `docs/decisions/README.md`.
- Constraints: the store schema is untouched (no new column; `env_landing`
  is a settings row; proposals reuse the `answer` + `decide` path); the
  hands stay a forced command with no `sudo`; every new command is read-only
  unless named here; a branch's recipe never configures its own check.
- Assumptions: `mise` is installable with one pinned release for the
  `pulse` user (verified checksum); the hub's local executor may lack `mise`
  and then reports unfit for recipes with `tools:` instead of failing;
  `project-light` is the first live project (its recipe would pin `uv` and
  list `make test` as `suggested_check:`, with `make` still the human's
  `apt-get`).

## Steps

Order follows the panel's advice (ADR-026 rule 8): the recipe, the local
installer, and `pulse env check` first so the first live proof comes early;
remote parity second; the human's `pulse decide` convenience third; the
dashboard and measurement last. The human gate on recipe landings is in
force from slice 1 (a held branch), not from slice 3.

### Slice 1 - the recipe, the local installer, the hold, `pulse env check`

- [ ] `docs/context/_template-environment.md`: frontmatter `recipe: 1`,
      `tools: []`, `setup: []`, `env: {}`, `services: []`,
      `suggested_check:` with commented examples, and prose explaining each
      key, the secret rule, that `suggested_check:` is advice for
      `verify_cmd`, and - in so many words - that a non-empty `services:`
      blocks every executor until a services decision exists. One line in
      `docs/context/README.md`.
- [ ] `pulse.py`: `RECIPE_PATH = "docs/context/environment.md"`,
      `parse_recipe(text) -> Recipe | RecipeError` (frontmatter only, exact
      key set, `recipe == 1`, `tools` as `name@version` matching the argv
      pattern of ADR-026 rule 1 with exact pins and `<= 12` entries, `setup`
      a list of strings, `env` a mapping of `^[A-Z_][A-Z0-9_]*$` names to
      single-line strings with the secret name and value patterns of rule 1
      refused - including `://user:pass@` URLs and `eyJ` three-part JWTs -
      `services` a list, `suggested_check` a string). Errors name the key
      and never echo an `env` value. `read_recipe(root, rev)` reads the file
      at the base commit with `git show rev:path` (absent -> `None`).
- [ ] `run_job`: after `ex.prepare()`, `recipe = read_recipe(clone, base)`;
      invalid -> the job errors before the worker with the message, and
      `recipe_invalid_finding(con, slug)` posts one `finding` a day on
      `runner/environment`; `services` non-empty -> `ExecutorUnfit(reason=
      "services")` for every executor (mark, requeue exactly as WU-021);
      `setup:` present -> replaces `setup_command`'s heuristic (joined with
      `set -e`, WU-021's `SETUP_ENV` prefix plus the recipe's `env:`
      exports; an explicit empty list runs nothing); `env:` exported for the
      check (`verify` string prefix) and for the worker on the **local**
      executor (subprocess env).
- [ ] Local `mise` path: `tools_install_command(recipe)` -> `mise install`
      with one argv element per pin, only the missing ones, under the setup
      cap; `mise_exec_argv(recipe)` -> `["mise", "exec", pin, ..., "--"]`
      prefixed to setup, worker, and check argv on the local executor;
      `mise` missing with a non-empty `tools:` -> `ExecutorUnfit(reason=
      "mise")` and the `tools-wanted` finding names the one install line; a
      pin that fails to install -> `ExecutorUnfit(reason="tool:<name>")`.
- [ ] Remote gate: a recipe with non-empty `tools:` or `env:` makes every
      **remote** executor unfit with reason `hands outdated` until slice 2's
      probe reports the new verbs - checked in `take_job` before placement,
      never after a partial run. `setup:` alone still runs remotely through
      the existing `verify` string.
- [ ] `write_brief(..., recipe=...)`: rule 3 names the recipe's setup lines,
      pins, and env names (not values) so the worker knows what already ran;
      the brief also states *recipe changes travel alone*.
- [ ] `CHECK_FILES += (RECIPE_PATH,)`: a branch that changes the recipe runs
      its landing check with the base's version (WU-021 rule, extended).
      `land_branch` reads the base recipe for the worktree setup.
- [ ] The hold: `HUMAN_LANDED_PATHS = (RECIPE_PATH,)`; in `land_branch`, a
      branch touching one and any other file is refused like a
      `LAND_REFUSED_PATHS` hit (`result`: *recipe changes travel alone*); a
      recipe-only branch returns `("held", note)`, stays, and posts one
      `question` on `runner/environment` naming it (once per branch); the
      human lands it with `pulse land --project P --branch B` or a hand
      merge. `land_pending` skips held branches.
- [ ] `cmd_onboard`: scaffold `docs/context/environment.md` from the template
      only when absent, printed with `git status --short`, never staged;
      `queue_onboarding_job`'s brief gains the recipe paragraph of ADR-026
      rule 2 (including *commit the recipe on its own*).
- [ ] `pair_apply_script` and `cloud-init.yaml`: install one pinned `mise`
      release for the `pulse` user with its checksum verified (fail the
      pairing step loudly on mismatch); `pair_inspect_script` reports
      `mise=` so `pulse pair` on an already paired machine adds it
      (idempotent, like the hands hash refresh).
- [ ] `pulse env check [--project P] [--executor E] [--json]`: read-only;
      recipe at the default branch; local executor's tools from
      `shutil.which` and `mise ls`; remote executors read `hands outdated -
      re-run pulse pair` (slice 2 fills the table); `suggested_check:`
      beside `verify_cmd`; the `services` warning; exit 1 when any executor
      is unfit for the project. `executors_line` and `pulse status` read
      `lacks uv`, `lacks mise`, `needs services`, `hands outdated`.
- [ ] Tests (`RecipeTestCase`): parser accepts the example, refuses unknown
      keys, `latest`, 13 tools, a pin with a space or `;`, a `GITHUB_TOKEN`
      name, `API_KEY_NAME`, a 40-char value, a `postgres://u:p@h/db` value,
      an `eyJ...` JWT, a multi-line value, `recipe: 2`; `run_job` prefers
      `setup:` over the heuristic, runs nothing for `setup: []`, exports
      `env:` for setup, worker, and check locally, wraps in `mise exec`
      argv; invalid recipe errors before the worker and posts one finding,
      not two; `services` marks unfit everywhere; remote executors are unfit
      `hands outdated` for `tools:`/`env:` recipes before placement; landing
      check uses the base recipe when the branch changed it; a mixed branch
      is refused, a recipe-only branch is held with one question;
      `onboard` scaffolds once and never overwrites; the brief carries
      setup lines, pins, env names, and the travel-alone rule; `env check`
      output and exit code; `pair` scripts contain the pinned `mise` line.
- [ ] Docs: `docs/workflows/executors.md` (the recipe paragraph under "What
      a machine needs installed"; `mise`; what stays `apt-get`),
      `docs/workflows/pulse.md` (run, landing, `env check`),
      `docs/context/pulse-store.md` (`unfit` reasons `tool:<name>`,
      `services`, `mise`, `hands outdated`), `cloud-init.yaml` header.

### Slice 2 - remote parity: the hands learn the recipe

- [ ] `pulse-hands.sh`: `probe` prints `verbs=tools,run-env` so the hub
      knows; new verb `tools NAME...` printing `name yes|no [version]` per
      line from `command -v` and `mise ls --json` (read-only, no install);
      `run` accepts extra keys whose names appear in `PULSE_RECIPE_ENV`
      (comma-separated, each `^[A-Z_][A-Z0-9_]*$`, the secret name pattern
      refused on this side too, duplicates refused) and, when `PULSE_TOOLS`
      is set and `mise` exists, validates every pin against the argv
      pattern and runs the copilot command through a `mise exec` **array**
      - never a string expansion; `verify` needs no change (the runner
      builds the shell string with `mise exec` from validated pins). Header
      comment lists the new verb; the forced-command posture is unchanged
      (no `sudo`, no remote, only copilot in `run`).
- [ ] `pulse.py`: `probe_executor` records the verbs; the `hands outdated`
      gate lifts for executors that report them; the remote path passes
      `PULSE_TOOLS` and `PULSE_RECIPE_ENV` to `run`, prefixes `verify`
      strings with `mise exec` built from validated pins, and installs
      missing pins through a `verify`-style `mise install` under the setup
      cap; `pulse env check` fills the table from the `tools` verb.
- [ ] Tests (`RecipeRemoteTestCase`): shim argv shows `mise install` then
      `mise exec ... -- sh -c`; `run` argv carries `PULSE_TOOLS` and
      `PULSE_RECIPE_ENV` as separate elements; env values with spaces reach
      the worker intact; the hands script refuses a `run` key not listed, a
      secret-shaped name, a duplicate key, and a malformed pin (`bash` tests
      through the shim harness); a probe without `verbs=` keeps the executor
      unfit; `bash -n pulse-hands.sh`.
- [ ] Docs: `executors.md`, `pulse.md`, the hands header.

### Slice 3 - the human's yes: proposals, `env_landing`, the incident rule

- [ ] `pulse.py`: the held branch becomes a proposal - on a recipe-only
      branch, run the landing check under the base recipe (advisory),
      validate the branch's recipe, dry-run its `setup:` in the worktree,
      run the project's check under the *proposed* recipe, and post one
      `answer` with `resolution: agreed` on `runner/environment` (branch,
      recipe diff, both outcomes, clean-count line) unless one is already
      pending for that branch; return `("proposed", note)`; `decide(...
      yes)` on that answer marks the job's branch `land_on_yes`; the
      daemon's `land_pending` lands such branches on the next loop regardless
      of the `landing` setting (the yes is the landing); `no` removes the
      branch and the `result` reads `declined`.
- [ ] Setting `env_landing` (`human` default, `agents`) in the defaults,
      `SETTING_RULES`, `settings_report`, and the dashboard settings
      whitelist (ADR-025); when `agents`, a recipe-only branch lands like
      any file under `landing`.
- [ ] Incident rule: a `setup` failure in a job whose base recipe landed
      within the last five jobs sets `env_landing` back to `human` (event
      `env.incident`), posts a `finding`, and the clean count restarts at 0.
- [ ] Tests (`RecipeLandingTestCase`): a recipe-only branch returns
      `proposed` with one answer carrying the diff and both outcomes, none
      the second time; a first recipe whose base check fails still proposes
      with the proposed-recipe line green; `decide yes` lands on the next
      `land_pending` even with `landing = human`; `decide no` removes the
      branch and records `declined`; `env_landing agents` lands directly;
      the incident flips the setting and posts the finding; settings
      validation for the new row.
- [ ] Docs: `pulse.md` (the third landing category), `pulse-store.md`
      (setting; events `env.proposed`, `env.landed`, `env.declined`,
      `env.incident`), `AGENTS.md` one line, memory when the first recipe
      lands on a paired machine.

### Slice 4 - watch and measure

- [ ] Measurement in `pulse status` and `docs/scripts/evolution-report.sh`:
      setup outcomes per project and executor, missing-tool and
      invalid-recipe errors, onboard-to-first-remote-green hours, recipe
      proposals landed and declined.
- [ ] Dashboard: read-only **Environment** card per project (tools, per
      executor fitness, last setup outcome, pending recipe proposal) from
      the store and `pulse env check --json`; no new write route.
- [ ] Tests: dashboard JSON for the card; the report's new lines.
- [ ] Docs: `dashboard.md`; ADR-026 `delivery:` mark and this plan's
      `delivered:` date.

## Rollback Plan

- **Baseline:** brain `main` at the commit that lands each slice (recorded
  in the slice's commit message); files above; the live store's `settings`
  rows (no schema change in any slice); `pulse-vm1`'s hands and `mise`
  install (slice 2). Adopter projects keep their own `docs/context/
  environment.md` under their own history.
- **Trigger:** the test suite fails after a slice; a job that passed before
  the slice errors on setup or the check with a recipe-less project (the
  heuristic fallback regressed); the daemon fails to re-exec on the new
  `pulse.py`; a `run` on a paired machine is refused by the new hands
  for a key it accepted before; a recipe lands without a recorded yes while
  `env_landing` is `human`; any `schema.upgrade` event from a `run-*` actor.
- **Reversal:** `git revert` the slice's commit(s) on the brain (one revert
  per slice, newest first); the daemon re-execs on the reverted `pulse.py`;
  for slice 2 re-run `pulse pair <admin>@<host> --name <executor>` so the
  hands return to the previous source (`mise` binaries under the `pulse`
  user's `~/.local` are inert and may stay; removing them is the human's
  `rm -rf ~/.local/share/mise ~/.local/bin/mise` as user `pulse`); for
  slice 3 `pulse settings set env_landing human` (or the row simply stops
  being read after the revert). Recipe files in adopter projects are left
  in place - they are data the runner ignores after the revert.
- **State safety:** no store migration in any slice, so no schema reversal;
  settings rows are plain values; branches waiting on a recipe proposal are
  ordinary job branches that `landing = human` behaviour already tolerates;
  installed tools live only under the executor user's home; nothing
  deployed, published, or merged into an adopter project by this plan
  itself (recipe landings are jobs under the existing landing rules).
- **Recovery verification:** `python3 -m unittest docs/scripts/tests/
  test_pulse.py` green; `python3 -m py_compile docs/scripts/pulse.py` and
  `bash -n docs/scripts/hosted/pulse-hands.sh`; `pulse status` shows the
  daemon alive with the reverted version; one queued job on a recipe-less
  project runs green under the heuristic; `pulse executor probe <name>`
  reports `hands=ok`.

## Acceptance Criteria

- [ ] A project with a valid recipe runs setup from the recipe (not the
      heuristic), exports its `env:` for setup, worker, and checks, and the
      landing check uses the base commit's recipe when the branch changed it.
- [ ] An invalid recipe never reaches the worker; the message names the
      key; no env value is echoed; one finding a day.
- [ ] `pulse onboard` scaffolds the recipe skeleton once; the onboarding
      brief asks for the filled recipe.
- [ ] A paired machine installs the pinned tools user-locally through
      `mise` and runs setup, worker, and checks with exactly those versions;
      a missing `mise` or a failed install marks the pair unfit with the
      reason and names the human's line.
- [ ] `pulse env check` shows every tool by executor and exits non-zero
      when any is unfit; `pulse status` reads the reason; an executor with
      old hands is unfit `hands outdated` before any job is placed there.
- [ ] A recipe change travels alone (a mixed branch is refused) and lands
      only after the human - a held branch landed by hand in slice 1,
      `pulse decide ... yes` from slice 3 - while `env_landing` is `human`;
      `no` removes the branch; `agents` lands it like any file; a setup
      incident flips the setting back.
- [ ] The dashboard shows the Environment card read-only; no new write route.
- [ ] Full suite green; `git diff --check` clean; docs updated; on closure
      `delivered:` set here and ADR-026 `delivery:` updated.

## Verification

- `python3 -m unittest docs/scripts/tests/test_pulse.py` (full suite; the
  new cases live in `RecipeTestCase`, `RecipeRemoteTestCase`,
  `RecipeLandingTestCase`).
- `python3 -m py_compile docs/scripts/pulse.py`;
  `bash -n docs/scripts/hosted/pulse-hands.sh`; `git diff --check`.
- Live, after slice 1: `pulse env check --project project-light` on the
  hub; a `project-light` recipe branch held with one `question`; the human
  lands it with `pulse land`; the next local job's setup runs from the
  recipe.
- Live, after slice 2: `pulse pair <admin>@vm1 --name pulse-vm1` (human),
  `pulse env check --project project-light` shows the table, one queued
  `project-light` job taken by `pulse-vm1` and green; `pulse status`
  executors line without `lacks uv`.
- Live, after slice 3: a recipe branch shows as `proposed` in
  `pulse proposals`; `pulse decide <id> yes` (human) lands it on the next
  daemon loop; the following job's setup passes.
- Walk every Edge Cases line of FEAT-016 against the code before ticking
  Acceptance Criteria.
