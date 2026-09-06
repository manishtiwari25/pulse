---
id: ADR-023
title: A Proper Frontend Stack for the Hub - Vite, TypeScript, the React API on Preact, Tailwind, and Radix, Built to Three Fixed Files the Read-Only Python Hub Serves
status: Accepted
accepted: 2026-09-06
date: 2026-09-06
delivery: partially delivered
delivered:
areas: [dashboard, dependencies]
tags: [adr, dashboard, hub, frontend, dependencies, build]
---

# ADR-023: A Proper Frontend Stack for the Hub

## Context

The dashboard and hub (`docs/scripts/pulse-dashboard.py`; ADR-015, ADR-016)
is one Python file: a read-only, loopback-only HTTP server over the store
with a hand-written page embedded as three strings - `INDEX_HTML`, `APP_CSS`
(~320 lines), `APP_JS` (~1,500 lines of DOM-building helpers: `el`, `kpi`,
`panel`, `table`, `chip`, a hash router, a theme toggle). It is honest and
dependency-free, and it has grown to the point where every visual change is
a hand edit of a string inside a Python module, with no types, no component
model, no design system, and no tooling beyond `node --check`.

The maintainer asked for the UI to look modern - something a user enjoys
and an investor recognises - and then, seeing the first CSS pass begin, said
plainly: **use a proper framework.** That is a dependency and build-tooling
decision for the product this repository ships, so it is Tier 2 and needs a
recorded choice.

What must not change, because earlier decisions and their owner-controlled
guards (`docs/scripts/tests/test_pulse_dashboard_guards.py`, an immutable
path in the constitution) hold the security posture:

- The hub binds `127.0.0.1` only, serves `GET`/`HEAD` only, writes nothing
  to the repository, and keeps its one write (`decide`) exactly as it is.
- Exactly three page assets - `/`, `/app.js`, `/app.css` - and the
  `/api/v1/*` JSON routes; no file serving, no path traversal.
- The Content-Security-Policy header stays verbatim: `default-src 'none';
  script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'
  data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. No
  inline script, no inline style element, no external request, ever.
- Board text is inserted as text, never as markup.
- `pulse-dashboard.py` imports only the Python standard library; a human
  runs the hub with `python3 docs/scripts/pulse-dashboard.py hub` and
  nothing else installed.

Prototype, 2026-09-06, outside the repository (`/tmp`, removed): Vite 7 +
TypeScript + Tailwind CSS 4 + Radix Tooltip and Tabs + lucide icons, built
with fixed output names to `index.html`, `app.js`, `app.css`, served by a
scratch server that set the hub's exact CSP and proxied `/api/v1/runner` to
the live hub. Rendered headless in Edge over the DevTools protocol:

| Runtime | `app.js` | gzipped | console errors | CSP violations | requests | forbidden literals in the bundle |
| --- | --- | --- | --- | --- | --- | --- |
| React 19 | 258 KB | 83 KB | 0 | 0 | 4, same-origin | `innerHTML` x5 (its `dangerouslySetInnerHTML` path); `https://react.dev/errors/` x2 |
| Preact 10 via `preact/compat` (same code, one alias) | 91 KB | 33 KB | 0 | 0 | 4, same-origin | `innerHTML` x3; no vendor URL (only W3C namespace strings) |

Both rendered the live numbers (`hands in standby 2`, `workers idle 3`,
`daemon alive`). Tailwind's CSS was 14.5 KB (3.5 KB gzipped). Build time
0.5 s; the toolchain installs in under a minute with a lockfile.

The one guard that cannot survive any framework is the **literal** check
that the served JavaScript never contains the string `innerHTML`: every
framework runtime carries that string in its own escape hatch, whether or
not the application uses it. The invariant behind the guard - the page never
turns board text into markup - is what must be kept; the check has to move
to where the decision is made: our source.

## Options Considered

### Option 1 - Keep the hand-written page and polish its CSS

- Pros: zero dependencies, zero build, every guard as it is; a few hours.
- Cons: the maintainer rejected it; the ceiling stays low (no types, no
  components, no design system); every future visual change is a string
  edit in a Python file.

### Option 2 - Server-rendered Python (Jinja + HTMX) with a vendored CSS framework

- Pros: stays Python; no Node.
- Cons: Jinja and HTMX are dependencies too, now on the *runtime* side,
  breaking "standard library only, one command"; a vendored CSS framework
  file is a dependency without a lockfile; HTMX swaps HTML fragments, which
  is exactly the markup-insertion the guards forbid.

### Option 3 - React 19 + Vite + TypeScript + Tailwind + Radix, built to fixed files

- Pros: the mainstream stack; the largest component ecosystem (shadcn/ui,
  Radix); TypeScript end to end; investors and hires recognise it.
- Cons: 258 KB / 83 KB gzipped for a local page; vendor error URLs in the
  bundle collide with the "never reach outside" guard and would need an
  allowance; no functional benefit over Option 4 for this page.

### Option 4 - The React API on Preact (`preact/compat`) + Vite + TypeScript + Tailwind + Radix, built to fixed files (chosen)

- Pros: identical code and ecosystem to Option 3 (shadcn/ui-style
  components, Radix primitives, lucide icons) at 91 KB / 33 KB gzipped; no
  vendor URL in the bundle so the outside-reach guard stays strict; one
  alias line switches to React if a component ever needs it; the Python hub
  stays standard-library-only because Node is a *build-time* tool and the
  built files are committed.
- Cons: a Node toolchain for developers (not for users); a committed build
  artifact that changes with every UI commit; the literal `innerHTML` guard
  must be rewritten as a source rule plus a runtime check; the constitution's
  product class and two evaluators need the owner's edit.

### Option 5 - Svelte 5 + Tailwind + bits-ui

- Pros: smallest runtime; excellent developer experience.
- Cons: smaller component ecosystem for this kind of console; the Svelte 5
  runtime instantiates templates through `innerHTML` and links
  `https://svelte.dev/e/` in its errors, so it needs the same guard changes
  as React with less ecosystem in return; not the API most contributors know.

## Decision

Adopt Option 4. The hub's page becomes a small, typed frontend application
built with **Vite 7, TypeScript, the React API on Preact (`preact/compat`),
Tailwind CSS 4, Radix UI primitives, and lucide icons**, with shadcn/ui-style
components copied into our source (owned code, no runtime component
library). It is **built to three fixed files** that the unchanged, read-only,
standard-library Python hub serves at the routes it already has.

### 1. Where it lives

```text
docs/scripts/dashboard/
  package.json, package-lock.json     the toolchain, pinned
  vite.config.ts, tsconfig.json, eslint.config.js
  index.html                          the shell: one script, one stylesheet, a data: favicon
  src/                                app code: routes, api client, components, design tokens
  dist/                               the committed build: index.html, app.js, app.css, BUILD.json
```

`node_modules/` is ignored. `dist/` is committed so that a human runs the
hub exactly as today with Python alone; `.gitattributes` marks `dist/**` as
`linguist-generated` and `-diff` so review noise stays out of pull
requests. `BUILD.json` records the source tree hash and tool versions the
build came from; a test fails when `dist/` is older than `src/`.

### 2. How it is served

`pulse-dashboard.py` reads `dist/index.html`, `dist/app.js`, `dist/app.css`
once at start and serves them at `/`, `/app.js`, `/app.css` with the same
headers and CSP as today. Nothing else under `dist/` or anywhere is ever
served; there is no static file handler. The `/api/v1/*` routes are the
contract between the two halves and do not change in this decision. While
the new page is reaching parity (PLAN-023 phase A) the hub serves the new
build when `dist/` is complete and the embedded legacy page otherwise; phase
B deletes the embedded page.

### 3. The invariants, restated for a framework

The owner-controlled guard module keeps: loopback only; GET/HEAD only; the
exact CSP string in the source and on every response; the exact security
headers; no file serving or traversal; the repository never written;
standard-library-only imports in the Python module; JSON payloads stay text.
Two checks move from the bundle to the source, where the decision is made:

- **Markup insertion:** `src/**` contains no `dangerouslySetInnerHTML`,
  `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `eval(`, `new Function`,
  or `document.write`; ESLint enforces `react/no-danger` in the UI project.
  The served bundle still must not contain `eval(`, `new Function`, or
  `document.write`; the runtime's own `innerHTML` string is tolerated
  because CSP and the source rule make it unreachable from board text.
- **Reaching outside:** `src/**` and `index.html` contain no `http://` or
  `https://`; the served bundle contains no `https://` and no `http://`
  other than W3C namespace strings; the render check (P4) asserts zero
  requests off `127.0.0.1` and zero CSP violations on every view.

These guard edits are a human commit (the module is an immutable path), made
when this ADR is accepted, before phase A lands.

### 4. Design language (the "modern" part, specified in FEAT-015)

Dark-first with a light theme, one accent, tabular numerals, hairline
borders on translucent panels, a sticky glass header, Radix-accessible
menus, tabs, tooltips and dialogs, `prefers-reduced-motion` respected, and
Mission control as the first screen an investor sees: the agents' machine,
the hands in standby, what is being tested now, what shipped. The design
tokens live in one CSS file under `src/` and stay `light-dark()`-based.

### 5. What the owner edits alongside acceptance

- Constitution class `dashboard (product)`: add
  `docs/scripts/dashboard/src/**`, `index.html`, and the config files;
  `dist/**` is generated, not a variable an experiment changes by hand.
- Evaluators: P2 becomes "`npm ci && npm run build` reproduces `dist/`
  byte-for-byte; `pulse-dashboard.py` stays stdlib-only; CSP unchanged";
  P6 measures `dist/index.html + app.js + app.css`; a new baseline row is
  recorded after phase B because the number changes meaning.
- Template-sync: `docs/scripts/dashboard/` (with `dist/`, without
  `node_modules/`) joins the sync-safe files so adopters need no Node.

### 6. Measurement

- P1, P3, P4 as today; P4 extended with the off-origin-request count.
- Bundle budget: `app.js` <= 150 KB gzipped, `app.css` <= 20 KB gzipped,
  checked by a test over `dist/`.
- Parity: every view in WU-005's verification list renders with the same
  data as the legacy page (phase A exit criterion), then the legacy strings
  are deleted and `pulse-dashboard.py` shrinks by ~1,900 lines.
- Accessibility: every interactive element reachable by keyboard; Radix
  primitives carry the ARIA; an axe pass over the rendered views reports no
  serious or critical issue.

## Consequences

### Positive

- A typed, componentised, testable UI with a design system; visual work
  becomes ordinary frontend work instead of string surgery in Python.
- The hub's contract - stdlib Python, loopback, read-only, strict CSP, three
  assets, JSON API - is unchanged; the guards get stronger where it matters
  (source rules plus a runtime request audit) instead of weaker.
- 33 KB gzipped of runtime; first paint well under a second locally.
- The React API and ecosystem without React's weight or vendor URLs; a
  one-line alias keeps the door to React open.

### Negative

- Node >= 20 and ~40 pinned development dependencies for anyone who changes
  the UI; a lockfile to maintain; a committed ~110 KB build that changes with
  each UI commit.
- Two evaluator rows and the constitution's class list need the owner's
  edit, and a new P6 baseline.
- Adopters receive the built files through template-sync; the UI source is
  in the template even though most adopters never touch it.

## Follow-Up

- [x] Human approval of this ADR (`status: Accepted`, `pulse decide` yes on
      `20260906T074412Z-fable-scale-09051203` at 2026-09-06T08:51:35Z); the
      Step 1 edits to the guard module, constitution class paths, and
      evaluators P2/P6 (§3, §5) landed as one separate commit the human can
      revert alone.
- [x] PLAN-023 phase A: scaffold, design tokens, components, Mission
      control and every view to parity, served by the hub when `dist/` is
      complete; legacy page as fallback (`--ui legacy`). A8 render: 76 cells,
      0 errors, 0 CSP blocks, 0 off-origin requests, identical KPI numbers;
      `app.js` 82 KB gzip of 150, `app.css` 7.5 KB of 20.
- [ ] PLAN-023 phase B: delete the embedded page; new P6 baseline row.
- [x] `docs/workflows/dashboard.md` and `docs/context/stack.md` describe the
      build, the fixed files, `--ui`, the stamp, the budget, and the guards.
- [x] Record memory: a framework's runtime always carries `innerHTML`; guard
      the source, not the bundle ([`docs/memory/patterns.md`](../memory/patterns.md),
      2026-09-06).
- [ ] Update `delivery:` and `delivered:` when PLAN-023 closes.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan or work-unit step has landed.
- `delivered` - the plan or last work unit closed; set `delivered: YYYY-MM-DD`.
- `retired` - the decision is superseded, deprecated, or abandoned.

Built by: [PLAN-023](../plans/PLAN-023-frontend-stack-for-the-hub.md),
specified by [FEAT-015](../features/015-modern-hub-ui.md).
