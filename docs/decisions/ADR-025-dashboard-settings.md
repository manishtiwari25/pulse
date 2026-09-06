---
id: ADR-025
title: Settings in the Dashboard - the Human's Caps Are Tweakable Where the Human Watches
status: Accepted
accepted: 2026-09-06
date: 2026-09-06
delivery: delivered
delivered: 2026-09-06
areas: [dashboard, runner]
tags: [adr, dashboard, settings, runner, caps, governance]
---

# ADR-025: Settings in the Dashboard

## Context

The runner's behaviour is a short list of settings rows in the store
(`jobs_per_day`, `minutes_per_job`, `workers`, `parallel_per_project`,
`max_ai_credits`, `quorum`, the amendment thresholds, `runner_cmd`,
`worker_model`, `landing`, `stop`). They are the human's caps: agents never
change them, the amendment vote never touches them, and until now the only
way to change one was the terminal - `pulse settings set NAME VALUE`.

The maintainer watches the machine through the dashboard and asked for the
caps there (chat, 2026-09-06, verbatim): *"have configuratrion in the UI so
it can be easily tweaked."* The dashboard was decided read-only
([ADR-015](ADR-015-project-board-observability-dashboard.md),
[ADR-016](ADR-016-pulse-organization-hub-and-commons.md)), and a single
exception was written later for the one thing only the human does:
`POST /api/v1/decide` ([ADR-017](ADR-017-democratic-board-human-veto.md),
[ADR-019](ADR-019-agent-first-pulse-store-and-runner.md)). A second write
route changes the dashboard's security posture again, which the hard stops
list, so it is Tier 2 and written down before the code.

The same week showed why a settings form needs rules of its own: the CLI
accepted any string (`pulse settings set workers two` fell back to the
default silently), and two settings rows the whitelist never knew
(`evolve_minutes`, `evolve_credits`) were written by a worker's modified
`pulse.py` against the live store before ADR-024's guards closed that hole.

## Options Considered

### Option 1 - Keep the terminal as the only way to change a setting

- Nothing changes in the dashboard's posture.
- The human keeps a second tool open to tune the machine they are watching,
  and the caps stay invisible to anyone reading the hub.

### Option 2 - One guarded settings route and a Settings view, the whitelist as the contract

- The same guards as `decide`: loopback only, an explicit header, JSON, a
  small body, the store opened read-write only for that one statement.
- One validation function in `pulse.py` that both the CLI and the route
  call, so a bad value fails the same way everywhere.
- The known settings, their defaults, bounds, and one-line descriptions come
  from `pulse.py`, so the form never knows a setting the runner does not.

### Option 3 - Let agents propose setting changes through the board and the human accept them in the UI

- Elegant, but the caps are the human's on purpose: agents already ask for
  more hands with a `finding` (WU-019) and the human decides; a proposal
  round for a number would slow the very thing the human wants to be quick.

## Decision

Option 2. The dashboard gains its second and last write route,
`POST /api/v1/settings`, and a read route, `GET /api/v1/settings`.

1. **Guards, identical to `decide`.** The server must be bound to
   `127.0.0.1`; the request carries `X-Pulse-Settings: yes` and
   `Content-Type: application/json`; the body is a JSON object of 1..16384
   bytes with `name` and `value`; the store is opened read-write for one
   `BEGIN IMMEDIATE` … `COMMIT`; any other method stays 405. The hub page
   served to a remote address shows the settings but cannot change them.
2. **The whitelist is the contract.** `DEFAULT_SETTINGS` in `pulse.py` is
   the only list of settings; each entry gets a rule (`SETTING_RULES`: kind,
   bounds, description). `validate_setting(name, value)` normalizes or
   raises, and `set_setting` calls it, so the CLI and the route reject the
   same values: counts are whole numbers at or above zero (`jobs_per_day`
   also takes `none`/`off`/`unlimited`), `max_ai_credits` never goes below
   the CLI's floor of 30, `amend_majority` is a fraction, `stop` is 0 or 1,
   `landing` is `agents` or `human`, `runner_cmd` is never empty.
3. **Provenance.** A change from the dashboard is recorded exactly like one
   from the CLI - the settings row's `updated_by` and a `setting` event -
   with the actor `human:dashboard`, so the log says where the number came
   from. Rows in `settings` that the whitelist does not know are shown to
   no one and touched by no one; a human removes them by hand if they wish.
4. **What the form says.** Every setting shows its value, default, bounds,
   the one-line meaning, and who changed it last. Saving one value is one
   request; the form confirms nothing and asks nothing - the human is the
   authority here. It notes once that the caps are the human's and that
   agents never change them.
5. **Hard stops unchanged.** The route changes numbers in the human's own
   store on the human's own machine. It merges nothing, deploys nothing,
   spends nothing, and holds no credential. The kill switch (`stop`) is
   flippable here because it is the human flipping it.

## Consequences

- The dashboard is "read-only except for what only the human does":
  `decide` and `settings`. Any third write route needs a new decision.
- Validation moves into `pulse.py` once, so the terminal gets stricter too:
  `pulse settings set workers two` now fails with the reason instead of
  silently doing nothing.
- The dashboard's `runner` payload carries the capacity note from WU-019,
  so "hands wanted" is visible where the caps are tuned.
- Tests cover the guards (remote bind, missing header, bad type, unknown
  name, out-of-bounds value), a successful change with its event, and the
  built UI staying in step with `dist/`.

## Rollback

- **Baseline:** the commit before WU-020 lands.
- **Trigger:** a setting changed without a matching `setting` event, a
  write accepted without the header or from a non-loopback bind, or a value
  outside its bounds reaching the store.
- **Reversal:** revert the WU-020 commit as one `git revert`; the settings
  rows themselves are plain data and stay valid for the CLI. Rebuild
  `dist/` from the reverted source and commit it.
- **Recovery check:** `python3 -m unittest discover -s docs/scripts/tests -p 'test_*.py'`
  passes; `POST /api/v1/settings` answers 405.

## Follow-up

- [WU-020](../plans/WU-020-dashboard-settings.md) built it (delivered 2026-09-06).
- When ADR-015 and ADR-016 are next pruned, their "never writes" line
  should read "writes only through the two guarded routes of ADR-017 and
  ADR-025".
