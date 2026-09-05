---
id: WU-004
title: Dashboard Mission-Control UI - Activity Chart, Richer Votes and Agents Views
status: Done
tier: 1
date: 2026-09-04
delivered: 2026-09-04
controlling_adr: ADR-015
areas: [agents, workflows, governance]
tags: [work-unit, dashboard, hub, ui, chart, votes, agents]
---

# Work Unit: Dashboard Mission-Control UI

One document for Tier 1 work: the plan, the behavior spec, and the
implementation prompt together. The dashboard and the hub already exist
(ADR-015, ADR-016); this work unit changes how they look and adds read-only
views the same decisions already allow.

## Tier Declaration

- **Tier:** 1 - new read-only views and a visual redesign inside the local
  dashboard that ADR-015 created and ADR-016 extended to the organization.
- **Controlling ADR:** [ADR-015: Project Board Observability Dashboard](../decisions/ADR-015-project-board-observability-dashboard.md),
  extended by [ADR-016: Organization Hub and Commons](../decisions/ADR-016-pulse-organization-hub-and-commons.md).
- **Why this stays inside the ADR:** ADR-015 draws the boundary as a
  read-only web UI on `127.0.0.1`, Python standard library only, no package
  manager, external JavaScript, CDN, telemetry, or network API, one
  computation path for votes (`board.sh`), and user text inserted only as
  text. Every change here is a new query over the existing SQLite projection
  or new client-side rendering of data the API already serves. No write
  route, no new store, no external asset, no second vote tally.
- **Hard stops touched:** none.

## Outcome

A maintainer who opens the hub sees a mission-control page: a KPI strip, a
line chart of board activity over time (24 hours, 7 days, 30 days) across
projects, who is working right now, the votes that need attention, and the
latest posts - all polished, responsive, keyboard-accessible, in light or dark
theme. The project overview gets the same chart for one project. Votes show
quorum progress, up/down split, and expiry; agents show call-sign collisions.

## Behavior

### User Story

As the maintainer of several PULSE projects, I want one modern page that
shows activity over time and what needs attention so that I can steer agents
without reading the Markdown board post by post.

### Requirements

- [x] Organization page (`/`): KPI strip; activity line chart (posts per
      hour for 24h, per day for 7d and 30d) with one line per project plus
      the total, hover values, a legend that toggles series, and one
      keyboard-focusable column per bucket that reads its values aloud;
      "working now" claims; "needs attention" votes (eligible first, then
      closest to quorum, then expiring soonest); latest activity feed;
      project cards with a 7-day sparkline.
- [x] Project overview: KPI tiles, the same chart for the one project, a
      kinds breakdown, then the existing sections.
- [x] Votes (`/#/votes`, project `#/suggestions`, `#/amendments`): quorum
      progress bar, up/down split bar, voters with direction and reason on
      hover, "expires in N days" for suggestions still needing votes,
      supermajority gauge with the 2/3 mark for amendments, filters by
      project and status, sort by queue order, closest to quorum, expiring
      first, or newest.
- [x] Agents (`/#/agents`): roster with avatar, call sign, a warning chip
      when one call sign sits on more than one agent id, runner/model,
      projects, counts, first/last seen, current claims; search box and
      "holding a claim" filter.
- [x] Activity (`/#/activity`): the chart on top, then the filtered feed with
      relative times (absolute on hover) and kind-colored markers.
- [x] Design system: CSS variables, dark theme by default with a light theme
      that follows `prefers-color-scheme` and a header toggle remembered in
      `localStorage`; visible focus, `prefers-reduced-motion` respected.
- [x] Nothing user-authored is ever inserted as markup; the CSP stays
      `default-src 'none'; script-src 'self'; style-src 'self'; …`.

### Interface Changes

- New read routes, all GET, all from the projection:
  `GET /api/v1/timeline` (organization: per-project and total series) and
  `GET <base>/api/v1/timeline` (one project). Response:
  `{ "now": "2026-09-04T14:00:00Z",
     "hours": [{"t": "2026-09-04T13", "n": 3, "kinds": {...}}, …],
     "days": [{"t": "2026-09-04", "n": 12, "kinds": {...}}, …],
     "total": 40, "before_window": 0, "unparsed": 0,
     "series": [{"slug": "alpha-app", "kind": "project", "base": "/p/alpha-app",
                 "days": [...], "hours": [...], ...}] }`
  - 24 hourly and 30 daily buckets ending at the projection's reference
  time (`board_params.now`), every bucket present even when empty; the
  `series` array exists only on the organization route, where every project
  is bucketed against one shared reference time so the lines align. A chart
  state can be linked with `#/<view>?range=24h|7d|30d&by=project|kind|total`.
- `GET /api/v1/roster` rows gain `name_shared` (boolean: this call sign is
  also used by another agent id) and `shared_with` (the other agent ids).
- `GET /api/v1/votes` suggestions gain `expiry_days` (from that project's
  `board_params`), so the client can show "expires in N days".
- No CLI change. No route is removed or renamed.

### Data Changes

None. The SQLite projection keeps its schema; the new routes are queries over
`posts`, `suggestions`, `amendments`, and `metadata`.

### Edge Cases

- A project with no posts: the chart shows an empty state, never a broken
  axis; KPI tiles show `0`.
- A registered project that is missing or fails to refresh: its card shows
  the error, its series is omitted from the chart, the rest renders.
- A snapshot older than the browser's poll: the client redraws only when the
  stamp changes, so filters, scroll, and focus survive (unchanged behavior).
- Very long summaries or names: truncated with an ellipsis and the full text
  in `title`; never rendered as markup.
- Dates outside the window: a future `date:` line (clock skew) is clamped
  into the last bucket; a post older than 30 days is counted in
  `before_window`; a date the parser cannot read is counted in `unparsed`
  (bare `YYYY-MM-DD` dates are read as midnight). The chart footnote shows
  both numbers, so buckets plus footnote always equal the post count, and the
  KPI tiles never depend on the chart window.
- `localStorage` unavailable (private mode): the theme toggle still works for
  the page lifetime; nothing else depends on it.

## Steps

- [x] Post a board `claim`; write this work unit.
- [x] Server: add `q_timeline(con)` and `q_org_timeline(hub)`, wire the
      `timeline` routes, add `name_shared`/`shared_with` to `q_roster`,
      `expiry_days` to `q_votes`.
- [x] Client: new CSS design system; chart component (inline SVG, no
      library); organization, votes, agents, activity, overview views;
      theme toggle; relative time helper.
- [x] Tests: timeline routes (project and organization), roster collision
      flag, votes `expiry_days`, static invariants (no `innerHTML`, no
      external URLs, `:focus-visible`, `prefers-color-scheme`).
- [x] `node --check` on the served JavaScript; browser check of every view
      against the fixture repositories and this machine's registry.
- [x] Update `docs/workflows/dashboard.md` (URL table, chart, theme, new
      fields); list WU-004 in `docs/plans/README.md`; close this work unit.

## Implementation Prompt

- **Repository and boundary:** `manishtiwari25/pulse`, the PULSE source
  repository. Everything lives in `docs/scripts/pulse-dashboard.py` (server,
  queries, and the embedded `INDEX_HTML`, `APP_CSS`, `APP_JS`) and its tests
  in `docs/scripts/tests/test_pulse_dashboard.py`. Keep the ADR-015 limits:
  standard library only, loopback only, GET only, no external assets, the CSP
  unchanged, `board.sh` the only tally.
- **Files likely to change:** `docs/scripts/pulse-dashboard.py`,
  `docs/scripts/tests/test_pulse_dashboard.py`,
  `docs/workflows/dashboard.md`, `docs/plans/README.md`, this file.
- **Patterns to reuse:** the `el()` helper (attributes set through the DOM,
  text through `textContent`), `who()` for identities, `statusChip()`, the
  `route()` hash router, `with_db()` and `hub.each()` for organization
  queries, the fixture builders in the tests (`Fixture`, `HubFixture`).
- **Constraints:** no `innerHTML`, `eval`, `https://`, or `http://` in the
  served page; styles set from JavaScript go through `style.cssText` or SVG
  attributes (the CSP allows CSSOM, not inline `style=""` attributes in
  HTML); every visible number must come from the projection, never from a
  second computation of votes.
- **Do not:** add a write route, human voting, a build step, a font or icon
  CDN, a chart library, or a new store; do not change `board.sh` or the
  evaluators; do not remove unrelated user work.

## Rollback Plan

- **Baseline:** `main` at `4068c8b`, working tree with two unrelated
  uncommitted edits from this session (`docs/workflows/board.md`,
  `docs/workflows/dashboard.md` one-sentence troubleshooting note) and the
  board finding `governance/call-sign-collisions`. Affected files are the
  five listed above. No deployment, no persisted data: the SQLite projection
  is disposable and lives in the user cache.
- **Trigger:** any existing dashboard test fails after the change, a served
  view throws in the browser, the CSP or the "no external asset" invariants
  break, or the maintainer rejects the design.
- **Reversal:** restore `docs/scripts/pulse-dashboard.py` and
  `docs/scripts/tests/test_pulse_dashboard.py` to `4068c8b` with
  `git checkout 4068c8b -- <file>` (single files, an exact inverse); remove
  this work unit, its `README.md` line, and the `dashboard.md` URL-table
  edits with an inverse patch; leave the unrelated edits and the board posts
  in place (the claim expires on its own; a `handoff` records the stop).
- **State safety:** no dependency, migration, or external state changes.
  Projections rebuild themselves from `SCHEMA_VERSION` on the next request;
  a stale one is discarded, never migrated.
- **Recovery verification:** `python3 -m unittest discover -s docs/scripts/tests`
  passes with the original 34 tests (37 after this work unit); `docs/scripts/check-links.sh` reports
  0 broken; the running hub answers `/api/v1/health` with `errors: {}`.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests` - 37 tests pass
  (the 34 existing ones plus `test_timeline_buckets_are_continuous_and_lose_nothing`,
  `test_timeline_route_loses_no_post`, and
  `test_hub_timeline_roster_collisions_and_vote_expiry`; the static-asset
  test now also requires `prefers-color-scheme`, `prefers-reduced-motion`,
  the theme toggle, and allows exactly one `http://` literal in the script,
  the SVG namespace identifier, which is never fetched).
- `APP_JS` extracted from the module and `node --check`: clean.
- `docs/scripts/check-links.sh`: 0 broken.
- Browser (2026-09-04): `pulse-dashboard.py hub --port 8767` against this
  machine's registry (three projects, 180 posts) rendered in headless
  Chromium at 1400 px and 820 px, light and dark (`preferredColorScheme`
  forced): `/`, `/#/votes`, `/#/agents`, `/#/activity`, `/#/activity?range=24h`,
  `/p/<slug>/`, `/p/<slug>/#/overview?range=24h&by=kind`, `#/threads`,
  `#/thread/...`, `#/post/...`, `#/suggestions`, `#/amendments`,
  `#/evolution`, `/commons/`, `/p/nope/` - console free of errors after two
  fixes found this way (array children passed to `replaceChildren`, and a
  `ResizeObserver` redraw that re-entered itself).

## Closure

- [x] Set `status: Done` and `delivered: 2026-09-04` in the frontmatter.
- [x] ADR-015 and ADR-016 already carry `delivery: delivered`; no change.
- [x] No new `docs/memory/` entry: the two lessons (CSP `style-src 'self'`
      blocks inline `style=""` attributes but not CSSOM writes from a script
      served by the same origin; `replaceChildren` does not flatten arrays)
      went to the board as a `finding` in
      `governance/dashboard-mission-control-ui`, where the next dashboard
      change will look first.
