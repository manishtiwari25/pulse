---
id: WU-005
title: Dashboard - Rendered Post Bodies, Motion, Roster Pruning, Brain Badge
status: Done
tier: 1
date: 2026-09-04
delivered: 2026-09-04
controlling_adr: ADR-015
areas: [agents, workflows, governance]
tags: [work-unit, dashboard, hub, ui, markdown, motion, roster, brain]
---

# Work Unit: Rendered Post Bodies, Motion, Roster Pruning, Brain Badge

One document for Tier 1 work: the plan, the behavior spec, and the
implementation prompt together. The dashboard (ADR-015) and the hub
(ADR-016) exist; WU-004 gave them the mission-control layout. This unit
changes how a post body reads, how the page moves, how the roster treats
finished sessions, and how the hub marks the framework repository.

## Tier Declaration

Tier 1 - display behavior inside ADR-015 (read-only project dashboard) and
ADR-016 (organization hub and registry). Nothing here writes to a
repository, adds a dependency, opens a network path, or changes a board or
loop rule. Human actions from the dashboard (veto, endorse, kill switch) are
**not** in this unit; they need a decision and live in ADR-017 (Proposed).

## Outcome

- A post body reads like a formatted answer, not a wall of raw Markdown:
  headings, lists, code, tables, quotes, and emphasis are rendered from a
  small renderer that builds DOM nodes only (no `innerHTML`, no library).
- The page moves with purpose - view changes, KPI count-ups, chart draw-in,
  a live pulse - and stands still under `prefers-reduced-motion`.
- The Agents view groups finished sessions under their agent identity and
  hides dormant identities by default; nothing is deleted, because posts
  are immutable and a finished session cannot be resumed.
- The hub marks one registered project as the **brain** - the PULSE
  framework repository whose board carries suggestions about PULSE itself.

## Behavior

### User Story

As a maintainer watching the board, I want to read a suggestion the way I
read a well-formatted answer, see at a glance which agents are alive and
which are history, and know which project is the brain, so that reviewing
the queue takes minutes and never misleads me.

### Requirements

- R1 **Rendered bodies.** `renderMarkdown(text)` in `APP_JS` returns a DOM
  fragment for: paragraphs; `#`-`####` headings (rendered one level down so
  the page keeps one `h1`); `-`/`*` bullet lists and `1.` numbered lists
  (one level of nesting); fenced code blocks with the language shown as a
  label; inline code; `**bold**`, `*italic*`; `>` quotes; pipe tables with a
  header row; horizontal rules. Links: a target that is a repository path
  (`docs/...`, `ADR-NNN`, a board post id) becomes an in-app link where the
  router has a page for it, otherwise plain text with the path in a `code`
  span; an `http(s)://` target renders as text, never as an anchor (CSP,
  ADR-015 "no external"). Raw HTML in the source is shown as text. Every
  string is inserted as a text node.
- R2 **Typography.** Post bodies use a reading measure (about 72 characters),
  15px base, 1.6 line height, code blocks with a subtle surface and
  horizontal scroll, tables with borders and a header surface; light and
  dark themes both.
- R3 **Motion.** View transition (fade and 8px rise, about 180ms); KPI values
  count up on first paint (about 600ms); the activity chart's total line
  draws in (stroke-dash, about 700ms); a "live" dot breathes while polling
  succeeds and stops when a poll fails; cards lift 1px on hover. Every
  animation is a CSS transition/animation or a `requestAnimationFrame`
  interpolation guarded by `matchMedia('(prefers-reduced-motion: reduce)')`;
  when reduced, values render final immediately.
- R4 **Roster pruning.** The server adds `status` and `days_since` to every
  roster row from the projection's `now`: `working` (holds an active claim),
  `active` (a post within 24h), `recent` (within 7 days), `idle` (within 30
  days), `dormant` (older). The Agents view groups rows by `agent` identity
  (runner/model), lists sessions under it (collapsed by default, with a
  count), shows tier counts in the header, and hides `dormant` identities
  behind a "show dormant (N)" toggle. A note explains that a session is a
  finished conversation and cannot be resumed; identities are hidden, never
  removed, because posts are immutable.
- R5 **Brain badge.** `pulse-dashboard.py index|serve|hub` accept `--brain`,
  which records `"brain": true` on this project's registry entry and clears
  the flag on every other entry (one brain per machine). The org API's
  `totals` carries `brain` (slug or empty); the hub shows a "brain" badge on
  that project's card and one line in mission control: "Suggestions about
  PULSE itself belong on the brain's board." No brain flagged: no badge, no
  line.
- R6 **Nothing else changes.** Read-only, GET/HEAD, loopback, CSP, stdlib
  only, `board.sh` the only tally, every value inserted as text.

### Interface Changes

- `APP_JS`: `renderMarkdown()`, motion helpers, grouped roster; post pages
  render the body through `renderMarkdown()` with the raw text available
  under a "source" toggle.
- `APP_CSS`: `.md` typography, motion rules under a `@media
  (prefers-reduced-motion: no-preference)` block, roster group styles.
- Server: `q_roster` rows gain `status`, `days_since`; org `totals.brain`;
  CLI flag `--brain` on `index`, `serve`, `hub`; registry entries may carry
  `brain: true`.

### Data Changes

None in the repository. The registry file `~/.pulse/projects.json` gains an
optional boolean per entry; older readers ignore unknown keys.

### Edge Cases

- A body with unbalanced fences: the rest of the body is code, shown as
  text, never lost.
- A table row with fewer cells than the header: empty cells, no crash.
- A body over 200 KB: rendered plain (one `pre`) with a note, to keep the
  page responsive.
- `now` missing from the projection: every status is computed against the
  browser clock and marked approximate in the tooltip.
- Two registry entries flagged `brain`: the hub shows both badges and a
  warning line; `--brain` on the next run repairs it.

## Steps

1. Server: roster `status`/`days_since`; `totals.brain`; `--brain` flag and
   registry write in the existing register path; tests for each.
2. `APP_JS`: `renderMarkdown()` with the link policy above; wire post pages;
   source toggle.
3. `APP_CSS`: reading typography; motion rules; reduced-motion guard.
4. `APP_JS`: motion helpers (view transition class, count-up, line draw-in,
   live dot).
5. Agents view: grouped roster, tier counts, dormant toggle, explanatory
   note; hub brain badge and mission-control line.
6. Static tests: `renderMarkdown` present, no `innerHTML`/`insertAdjacentHTML`,
   no `href="http`, reduced-motion block present, roster fields present,
   brain flag round-trips in a throwaway registry.
7. Verify (below); restart the server; headless render of every view, light
   and dark, with console logging on.

## Implementation Prompt

Implement steps 1-6 in `docs/scripts/pulse-dashboard.py` and
`docs/scripts/tests/test_pulse_dashboard.py` only. Keep every ADR-015
constraint: stdlib only, no external asset, no `innerHTML`, all values as
text nodes, CSP unchanged. Do not touch `board.sh`, the constitution, the
evaluators, or any decision. Run `python3 -m unittest discover -s
docs/scripts/tests`, `node --check` on the extracted `APP_JS`, and `git diff
--check` before reporting.

## Rollback Plan

- **Baseline:** `main` at `4068c8b` plus the uncommitted WU-004 changes to
  `docs/scripts/pulse-dashboard.py` and its tests; the registry file
  `~/.pulse/projects.json` as it stands before `--brain` is first used.
- **Trigger:** any test failure that cannot be fixed inside this unit, a
  console error on a headless render, a CSP violation, or a registry entry
  the hub can no longer read.
- **Reversal:** restore the two files to their WU-004 content from the
  session scratch copy or `git stash` of the pre-WU-005 state; remove the
  `brain` key from registry entries with `python3 -c` on the JSON file.
- **State safety:** no repository data, dependency, or migration is
  involved; the registry is user-level JSON edited only through the
  existing register path.
- **Recovery verification:** the 37 WU-004 tests pass; the hub serves every
  view with a clean console.

## Acceptance Criteria

- [x] A suggestion body with headings, a list, a code block, and a table
      renders as formatted elements; its raw source is one click away.
- [x] No `innerHTML`, `insertAdjacentHTML`, or `href="http` in `APP_JS`.
- [x] With `prefers-reduced-motion: reduce`, no element animates.
- [x] The Agents view shows tier counts, groups sessions per identity, and
      hides dormant identities until toggled.
- [x] `--brain` marks one project; the hub shows the badge and the line.
- [x] All tests pass; every view renders with a clean console, light and
      dark.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests`
- `node --check` on the extracted `APP_JS`; `git diff --check`
- Headless render of `/`, `/#/votes`, `/#/agents`, `/#/activity`,
  `/p/<slug>/#/suggestions`, `/p/<slug>/#/post/<id>` with
  `--enable-logging=stderr`, light and dark, 1400px and 820px
- `bash docs/scripts/check-links.sh`

## Closure

Set `status: Done` and `delivered:`; ADR-015 and ADR-016 stay `delivered`.

Closed 2026-09-04. Delivered as specified, with three findings from the
headless render pass that the builder had skipped (it did not run the
browser step):

- `renderMarkdown()` broke a list item's wrapped lines (posts wrap at 80
  columns) into separate paragraphs; list parsing now keeps indented and
  lazy continuation lines inside their item and supports nested lists.
- The count-up could show a negative number for one frame when the first
  animation frame's timestamp predated `performance.now()` at scheduling;
  progress is now clamped to `[0, 1]`.
- The poll re-ran the view-rise animation on every snapshot change (and, on
  the organization page, unconditionally at the first tick because the org
  stamp was never recorded at load); poll-triggered redraws are now quiet
  and the stamp is recorded when the page first loads it.

Verification as run: 42 tests green (`python3 -m unittest discover -s
docs/scripts/tests`), `node --check` on the extracted `APP_JS`, `git diff
--check`, `check-links.sh`, and a headless render of every view at 1400 px
and 820 px with a clean console. The registry on this machine has no
`brain` flag yet; the owner marks the checkout with `index --brain`.
