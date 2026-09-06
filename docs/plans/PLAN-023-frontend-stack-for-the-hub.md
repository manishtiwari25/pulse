---
id: PLAN-023
title: A Proper Frontend Stack for the Hub - Scaffold, Reach Parity, Retire the Embedded Page
status: In Progress
tier: 2
date: 2026-09-06
delivered:
tags: [plan, dashboard, hub, frontend, dependencies, build]
---

# Plan: A Proper Frontend Stack for the Hub

## Tier

Tier 2 - a new build toolchain and pinned dependencies for the product this
repository ships, plus an owner-controlled guard revision. Decision:
[ADR-023](../decisions/ADR-023-frontend-stack-for-the-hub.md). Behavior:
[FEAT-015](../features/015-modern-hub-ui.md). Nothing in this plan starts
before ADR-023 is `Accepted`.

## Goal

The hub's page is a typed Vite + TypeScript + Preact (`preact/compat`) +
Tailwind 4 + Radix application under `docs/scripts/dashboard/`, built to
`dist/index.html`, `dist/app.js`, `dist/app.css`, served by the unchanged
standard-library Python hub at `/`, `/app.js`, `/app.css`, looking the way
FEAT-015 specifies, with every existing view at parity, every guard green,
and the embedded legacy page deleted.

## Context

- Product: `docs/scripts/pulse-dashboard.py` (server + embedded page),
  `docs/scripts/tests/test_pulse_dashboard.py` (editable tests),
  `docs/scripts/tests/test_pulse_dashboard_guards.py` (owner-controlled,
  immutable path), `docs/evolution/constitution.md` and `evaluators.md`
  (owner-controlled), `docs/workflows/dashboard.md`.
- API contract: `/api/v1/health|org|notices|proposals|roster|executors|
  runner|votes|activity` at the hub root and `/p/<slug>/api/v1/overview|
  threads|threads/<key>|posts/<id>|suggestions|amendments|evolution|
  timeline|refresh` per project. Unchanged by this plan.
- Constraints that do not move: loopback only, GET/HEAD only, exact CSP,
  three assets, no file serving, repository never written, Python stdlib
  only, board text inserted as text.
- Toolchain on the maintainer's machine: Node 22.18, npm 11.5; prototype
  numbers in ADR-023.
- The hub is running on port 8765 for the maintainer; every phase ends with
  a restart of that hub so the change is visible.

## Steps

### Step 1 - The owner's edits (human commit, on acceptance)

- [x] `test_pulse_dashboard_guards.py`: replace the bundle-literal
      `innerHTML`/`outerHTML`/`insertAdjacentHTML` check with the source
      rule over `docs/scripts/dashboard/src/**` and `index.html`; keep
      `eval(`, `new Function`, `document.write` on the bundle; keep the URL
      rule on the bundle with W3C namespaces allowed and add the same rule
      on the source; everything else unchanged. The agent prepares this
      diff as a patch file in the session workspace and quotes it in the
      board proposal; the human applies and commits it.
- [x] Constitution: class `dashboard (product)` paths gain
      `docs/scripts/dashboard/src/**`, `docs/scripts/dashboard/index.html`,
      `vite.config.ts`, `tsconfig*.json`, `eslint.config.js`,
      `package.json`; `package-lock.json` and `dist/**` are generated
      outputs of the build step, listed as such.
- [x] Evaluators: P2 and P6 as ADR-023 §5; note that a new P6 baseline is
      recorded at phase B. (Landed as one commit after the human's yes,
      revertable alone; see the ADR Follow-Up.)

### Phase A - Scaffold and parity (agent, standing autonomy inside ADR-023 once Accepted)

- [x] A1 Scaffold `docs/scripts/dashboard/` with Vite 7, TypeScript strict,
      `@preact/preset-vite`, `@tailwindcss/vite`, Radix primitives (tooltip,
      tabs, dialog, dropdown-menu, popover, switch), `lucide-react`,
      `class-variance-authority`, `clsx`, `tailwind-merge`, ESLint with
      `react/no-danger`; fixed output names; `modulePreload.polyfill: false`;
      `cssCodeSplit: false`; `index.html` with one script, one stylesheet,
      `<link rel="icon" href="data:,">`, `<meta name="color-scheme">`;
      `.gitignore` for `node_modules/`; `.gitattributes` for `dist/**`.
- [x] A2 Design tokens in `src/styles/tokens.css` (`light-dark()`, one
      accent, semantic ok/warn/bad/info, post-kind colours, radii, shadows)
      and Tailwind `@theme` mapping; `prefers-reduced-motion` rules.
- [x] A3 API client (`src/api.ts`) typed from the JSON the routes return
      today (types written from live responses; no server change), with the
      freshness header and error states the legacy page has.
- [x] A4 Components: `Kpi`, `Panel`, `DataTable` (sticky header, sortable,
      keyboard rows), `Chip`, `KindDot`, `Sparkline` (SVG), `Gauge`,
      `Split`, `Seg`, `ThemeToggle`, `Notice`, `Notices`, `EmptyState`,
      `Skeleton`; all text through JSX text nodes only.
- [x] A5 Views, hub: Mission control (`/`), Votes, Agents (roster,
      executors, the agents' machine), Activity, Proposals. Views, project
      (`/p/<slug>/`): Overview, Threads, Thread, Post, Suggestions,
      Amendments, Evolution, Timeline. Hash routes identical to today so
      bookmarks and `pulse` output links keep working.
- [x] A6 `pulse-dashboard.py`: read `dist/` at start; serve the three files
      at the existing routes when all three exist and `BUILD.json` parses,
      else the embedded page; log which one at start; a `--ui legacy|dist`
      flag for the maintainer. No other route added.
- [x] A7 Tests (`test_pulse_dashboard.py`): dist served with the exact
      headers; missing dist falls back; `BUILD.json` source hash equals the
      hash of `src/**` + `index.html` + configs (fails when `dist/` is
      stale); bundle budget (`app.js` <= 150 KB gz, `app.css` <= 20 KB gz);
      source rules (no markup insertion, no URLs) as an editable twin of the
      guard so the loop sees them too.
- [x] A8 Render check: extend the P4 renderer to count off-origin requests;
      run every WU-005 view, light and dark, 1400 and 820, on both pages;
      exit criterion: 0 errors, 0 CSP violations, 0 off-origin requests, and
      the same headline numbers as the legacy page for the same store.
- [x] A9 Docs: `docs/workflows/dashboard.md` (build, `--ui`, budget),
      `docs/context/` note on the frontend; ADR-023 `delivery: partially
      delivered`. Restart the hub on 8765. Board `result`.

### Phase B - Retire the embedded page

- [ ] B1 Delete `APP_CSS`, `APP_JS`, the legacy `INDEX_HTML`, the `--ui`
      flag and the fallback; the hub fails at start with a plain message if
      `dist/` is incomplete.
- [ ] B2 Tests that referenced `APP_JS`/`APP_CSS` move to `dist/`;
      `evolution-measure.py product` reads `dist/` for P6; the owner
      records the new P6 baseline row.
- [ ] B3 Template-sync: `docs/scripts/dashboard/` (without
      `node_modules/`) joins the sync-safe files; `.template-sync` notes.
- [ ] B4 ADR-023 `delivery: delivered`, this plan `delivered:`, FEAT-015
      `delivered:`; memory entry ("guard the source, not the bundle").

## Rollback Plan

- **Baseline:** `main` at `553b14a` (the commit before Step 1 and phase A); `pulse-dashboard.py`, its tests, `docs/workflows/dashboard.md`,
  and the new `docs/scripts/dashboard/` tree are the only files touched.
  The store, the runner, `pulse.py`, and `pulse-hands.sh` are not involved.
- **Trigger:** any guard test red; P4 render with an error, a CSP violation,
  or an off-origin request; a bundle over budget; a WU-005 view that shows
  different numbers from the legacy page for the same store; the hub failing
  to start with `dist/` present.
- **Reversal (phase A):** `python3 docs/scripts/pulse-dashboard.py hub --ui
  legacy` restores the old page immediately with no code change; then
  `git revert` of the phase A commits removes the tree. **Reversal (phase
  B):** `git revert` of the phase B commits restores the embedded page and
  the fallback.
- **State safety:** no migration, no store change, no external state;
  `node_modules/` never enters Git; the committed `dist/` is regenerated by
  `npm ci && npm run build` and checked by the stale-build test; a revert
  leaves the running hub untouched until its restart.
- **Recovery verification:** `python3 -m unittest discover -s
  docs/scripts/tests` green; `python3 -m unittest
  docs.scripts.tests.test_pulse_dashboard_guards` green; the P4 render on
  the restored page 0/0/0; the hub on 8765 serving the restored page.

## Acceptance Criteria

- [ ] ADR-023 `Accepted` and the owner's Step 1 edits committed before any
      phase A file exists in `main`.
- [x] The Python hub still imports only the standard library; a human runs it
      with `python3 docs/scripts/pulse-dashboard.py hub` and nothing else.
- [x] Exactly three page assets served, exact CSP and headers, no file
      serving; guards green.
- [x] Every WU-005 view at parity, light and dark, two widths; P4 0 errors,
      0 CSP violations, 0 off-origin requests.
- [x] Bundle within budget; `dist/` matches `src/` by hash.
- [ ] FEAT-015's acceptance criteria met; the maintainer has seen the hub
      on 8765 and said it looks right.
- [ ] On closure, `delivered:` is set here and ADR-023's `delivery:` mark is
      updated.

## Verification

- `cd docs/scripts/dashboard && npm ci && npm run lint && npm run build`
- `python3 -m unittest discover -s docs/scripts/tests`
- `python3 -m unittest docs.scripts.tests.test_pulse_dashboard_guards`
- P4 renderer (session workspace `p4-cdp.mjs`, extended per A8) over both
  pages, both schemes, 1400 and 820
- `python3 docs/scripts/evolution-measure.py product --label <commit>`
- `bash docs/scripts/check-links.sh`; `git diff --check`
- Walk every Edge Cases line of FEAT-015 against the code before ticking
  Acceptance Criteria.
