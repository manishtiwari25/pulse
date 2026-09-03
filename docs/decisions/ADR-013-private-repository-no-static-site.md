---
id: ADR-013
title: Keep the Repository Private and Retire the Static Site
status: Accepted
date: 2026-09-03
delivery: delivered
delivered: 2026-09-03
areas: [publishing, documentation, hosting]
tags: [adr, private, github-pages, documentation]
---

# ADR-013: Keep the Repository Private and Retire the Static Site

## Context

[ADR-003](ADR-003-pulse-brand-and-public-pages.md) gave PULSE a brand and a
dependency-free GitHub Pages portal (`docs/index.html`,
`docs/skills/index.html`, `docs/assets/pulse.css`, `docs/.nojekyll`) as the
public face of the framework. The maintainer has decided to make the
repository private. A public site for a private repository has no readers,
and every framework change so far has had to be mirrored into the HTML by
hand - the tiers, the pitfalls guide, and the versions page each cost a
second edit in two pages that nothing verified.

## Options Considered

### Option 1: Keep the site files, stop publishing

- **Pros:** nothing to delete; the site could return later.
- **Cons:** two unverified HTML mirrors of the docs keep drifting; agents
  keep spending effort on a surface nobody sees.

### Option 2: Remove the site, keep the brand

- **Pros:** one source of truth again - the Markdown control plane; less to
  keep in sync; the SVG mark still gives the README and adopters an identity.
- **Cons:** the history in ADR-003 and the v2 plans refers to files that no
  longer exist; a future public release would rebuild the site from the docs.

### Option 3: Remove the brand too

- **Pros:** smallest repository.
- **Cons:** the brand is cheap to keep and is used by the README and adopting
  repositories; no reason to lose it.

## Decision

**Option 2.** The repository is private. `docs/index.html`,
`docs/skills/index.html`, `docs/assets/pulse.css`, and `docs/.nojekyll` are
removed; `docs/assets/pulse-logo.svg` stays. ADR-003 is marked Superseded
with `delivery: retired`; its brand decision remains in force through this
ADR. Live documents no longer link to the site. Historical records (ADR-003's
body, the v2 plans, ADR-011's follow-up list) keep their original text as the
record of what was done at the time.

Turning the repository private and disabling GitHub Pages are repository
settings the maintainer changes; agents do not change hosting settings.

## Consequences

### Positive

- One source of truth; no HTML mirror to forget.
- Agents' effort goes to the control plane and the skills adopters actually
  install.

### Negative

- If PULSE is ever published again, the site is rebuilt from the docs rather
  than restored, and ADR-003's portal decision would need a new ADR.
- `gh skill install` from this repository requires a GitHub CLI login with
  access.

## Follow-Up

- [x] Remove the site files and fix live references.
- [x] Mark ADR-003 Superseded, `delivery: retired`.
- [ ] Maintainer: set the repository to private and disable GitHub Pages in
  the repository settings.
- [x] Record the retirement in `docs/versions.md` under v5.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan or work-unit step has landed.
- `delivered` - the plan or last work unit closed; set `delivered: YYYY-MM-DD`.
- `retired` - the decision is superseded, deprecated, or abandoned.

Built by: the same change that delivered ADR-012; no separate plan. The
remaining follow-up is a repository setting, not repository content.
