---
id: ADR-003
title: Adopt the PULSE Brand and Static GitHub Pages Portal
status: Accepted
date: 2026-08-11
areas: [framework, branding, documentation, publishing]
tags: [adr, pulse, github-pages, svg, documentation]
---

# ADR-003: Adopt the PULSE Brand and Static GitHub Pages Portal

## Context

The framework had a generic template identity and no public documentation
entry point. Users need a name they can remember, a clear lifecycle, and one
public page that leads to the versioned source of truth.

## Decision

1. Name the framework **PULSE**, meaning **Planning & Unified Lifecycle for
   Software Engineering**.
2. Use "PULSE control plane" for the canonical `docs/` structure.
3. Rename the public repository to `manishtiwari25/pulse`.
4. Publish `docs/index.html` through GitHub Pages from the default branch's
   `/docs` folder.
5. Keep the site dependency-free with plain HTML, CSS, and an original SVG.
6. Link detailed pages back to versioned Markdown in the public repository.

## Consequences

### Positive

- The framework has one durable public identity.
- The documentation portal works without a package manager or build service.
- The site and repository stay easy to inspect, fork, and adopt.
- The SVG can be reused in the README and public page.

### Negative

- Repository URLs and bootstrap launchers must be updated together.
- The static portal must be maintained by hand when the lifecycle changes.
- The public site depends on GitHub Pages availability.

## Follow-Up

- [x] Update framework instructions and context for PULSE.
- [x] Create the SVG and static public portal.
- [x] Rename the repository and enable GitHub Pages.
- [x] Verify the live page and set it as the repository homepage.
