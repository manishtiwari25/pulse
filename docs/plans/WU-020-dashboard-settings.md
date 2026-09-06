---
id: WU-020
title: Settings in the Dashboard - One Guarded Route, the CLI's Own Rules, a Settings Tab
status: Done
tier: 2
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-025
areas: [dashboard, runner]
tags: [work-unit, dashboard, settings, runner, caps, capacity]
---

# Work Unit: Settings in the Dashboard

## Tier Declaration

- **Tier:** 2 - a new write surface on the dashboard, which no ADR allowed
  before; the caps it edits are the human's governance numbers (ADR-019).
  The decision is [ADR-025](../decisions/ADR-025-dashboard-settings.md),
  accepted by the maintainer in chat on 2026-09-06 ("have configuration in
  the UI so it can be easily tweaked"; "you dont have to ask me man, please
  make everything agent driven"). This unit is the plan, behaviour spec, and
  implementation record in one file; no separate feature spec or prompt was
  needed because the behaviour is one form over one route.
- **Controlling ADR:** [ADR-025: Settings in the Dashboard](../decisions/ADR-025-dashboard-settings.md),
  on top of [ADR-019](../decisions/ADR-019-agent-first-pulse-store-and-runner.md)
  (the store, the settings rows, the decide route's guards) and
  [ADR-015](../decisions/ADR-015-project-board-observability-dashboard.md) /
  [ADR-016](../decisions/ADR-016-pulse-organization-hub-and-commons.md)
  (the dashboard and hub the tab lives in).
- **Hard stops touched:** none. The route is loopback-only and human-only
  (agents never call it; no vote reaches it); a cap is a number the runner
  works under, never permission to cross a hard stop. Creating a machine,
  merging, pushing, and credentials stay outside the dashboard.

## Context

WU-019 made the runner faster and taught it to say when a machine would
help. The knobs it works under - `parallel_per_project`, `workers`,
`jobs_per_day`, `minutes_per_job`, `max_ai_credits`, the kill switch, the
worker's program and model, `landing`, the board timers, the amendment
thresholds - could only be changed from a terminal with
`pulse settings set`, and the CLI accepted any string (a typo such as
`parallel_per_project abc` was stored and silently read as the default).
The maintainer watches the machine in the dashboard and asked for the
knobs to be there too.

## Behaviour

- `GET /api/v1/settings` (hub root and under every `/p/<slug>/`) returns
  `available`, `can_edit`, and one row per whitelisted setting: `name`,
  `value`, `default`, `kind`, `rule` (plain-word hint), `description`,
  `group`, `updated_at`, `updated_by`. Rows the whitelist does not know are
  not listed. Without a store: `available: false` and an empty list.
- `POST /api/v1/settings` with `X-Pulse-Settings: yes`, a JSON object
  `{"name","value"}` under 16 KiB, on the loopback listener, with the store
  present, runs `validate_setting` (the same function `pulse settings set`
  runs) and writes the normalized value with `updated_by = human:dashboard`
  in one `BEGIN IMMEDIATE` transaction, then refreshes the projection and
  returns `{"ok": true, "setting": <row>}`. A refused value returns the
  CLI's error text as `{"error": ...}` and writes nothing. Guard failures
  answer exactly as the decide route does (403 header, 415 content type,
  400 body, 409 no store, 405 elsewhere).
- `/#/settings` shows the rows grouped (the machine, the worker, the board,
  amendments) with value, rule hint, default, and provenance; Save is
  disabled until the value changes; `stop` is a two-state button; `landing`
  a select; errors from the server appear inline; everything is disabled
  when `can_edit` is false (Markdown-only mode or a non-loopback hub).
- The machine panel on Mission control shows the `capacity` line from
  WU-019 with a "hands wanted" chip when the daemon says another machine
  would help.

## Steps

1. `pulse.py`: `SETTING_RULES` (kind, bounds, description, group for every
   `DEFAULT_SETTINGS` name), `validate_setting(name, value)` that normalizes
   or raises `PulseError`, `settings_report(con)`, and `set_setting`
   validating first - so the CLI refuses the same values the dashboard does.
2. `pulse-dashboard.py`: `q_settings`, `q_runner.capacity`, the shared
   `_write_payload` guard (extracted from the decide route), `_settings_api`,
   `_refresh_after_write`, and the GET route on both the hub and project
   APIs.
3. UI (`docs/scripts/dashboard/src`): `Setting`/`SettingsPayload` types and
   per-route write headers in `api.ts`; `views/settings.tsx`; the Settings
   entry in `header.tsx`; the capacity line in `cards.tsx`; the footer and
   server banner now say two writes. `npm run build` rebuilt `dist/`.
4. Tests: `ServerTestCase.test_settings_route_without_store_says_so`;
   `StoreModeServerTestCase` lists every known setting with its rule,
   changes one cap with provenance, refuses what the CLI refuses, and shares
   the decide guards; the 405 message test names both write routes; the
   stricter setter is covered in `test_pulse.py`.
5. Docs: this unit, ADR-025 marks, `workflows/dashboard.md` (the second
   write, the capacity line), `decisions/README.md`, `plans/README.md`.

## Acceptance Criteria

- Every `DEFAULT_SETTINGS` name appears once in `GET /api/v1/settings` with
  a rule; orphan rows do not.
- `POST` with `{"name":"parallel_per_project","value":"3"}` changes the
  store row, records `human:dashboard`, and the daemon's next lease honours
  it without a restart; `{"value":"abc"}` is refused with the CLI's text.
- `pulse settings set parallel_per_project abc` exits 2 with the same text.
- All other `POST`/`PUT`/`PATCH`/`DELETE` paths still return 405.
- The full check passes and `BuiltUITestCase` finds `dist/` fresh.

## Verification

- `cd docs/scripts/tests && python3 -m unittest test_pulse_dashboard`
  (65 tests, 1 skip) and the full suite
  `python3 -m unittest discover -s docs/scripts/tests -p 'test_*.py'`.
- Live on this machine: `curl -s http://127.0.0.1:8765/api/v1/settings`
  lists 16 rows with `can_edit: true`; `/#/settings` renders every group;
  Mission control's machine panel shows the two executors and the capacity
  line.

## Rollback

- **Baseline:** commit `90a5b53` (WU-019) before this unit.
- **Trigger:** a setting saved from the dashboard that the CLI would have
  refused, a write from a non-loopback address, or a stale `dist/` failing
  `BuiltUITestCase`.
- **Reversal:** `git revert <sha>` of this unit's commit (one commit,
  including `dist/`), then `npm run build` and commit the rebuilt `dist/` if
  the revert leaves it stale. No reset, no history rewrite.
- **State safety:** no schema change; settings rows written by the dashboard
  are ordinary rows with `updated_by = human:dashboard` and stay valid after
  a revert; the daemon keeps reading them.
- **Recovery check:** `POST /api/v1/settings` returns 405 again and the full
  suite passes.

## Learning

- A write route the human already trusts (decide) is the right template for
  the next one: the same guards, the same CLI function behind it, one actor
  label. Validation had to move into `pulse.py` first so the terminal and the
  page could never disagree about what a valid cap is.
