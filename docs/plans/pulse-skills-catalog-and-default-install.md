---
id: PLAN-004
title: Publish the PULSE Skills Catalog and Include Every Skill by Default
status: In Progress
date: 2026-08-14
tags: [plan, skills, website, bootstrap, distribution]
---

# Plan: Publish the PULSE Skills Catalog and Include Every Skill by Default

## Goal

Give PULSE skills a dedicated public catalog in the existing PULSE visual
language and make all eight canonical skills part of every PULSE installation.

## Context

- The canonical bundles already live under `docs/skills/`.
- GitHub CLI and the universal `skills` CLI both discover all eight bundles.
- `npx skills@latest add manishtiwari25/pulse --skill '*' --agent
  github-copilot --copy --yes` was verified in an isolated project.
- A PULSE bootstrap currently treats portable skills as optional; this must
  become a default framework guarantee.
- Generated runner directories remain local installation output and must not
  become a second committed source.

## Steps

- [x] Verify universal and GitHub CLI discovery of all eight skills.
- [x] Create a dedicated PULSE-styled catalog at `/pulse/skills/`.
- [x] Show one-command full-pack installation plus supported alternatives.
- [x] Update bootstrap guidance so every PULSE install includes `docs/skills/`.
- [x] Update the homepage and Markdown entry points to the new catalog.
- [x] Validate installers, links, HTML, and responsive layout locally.
- [ ] Publish to `main` and verify the live catalog.

## Rollback Plan

- **Baseline:** clean `main` at commit `1cca8e4`, with the existing homepage
  skills section and all eight source bundles already published.
- **Trigger:** broken skill discovery, incorrect install guidance, invalid
  markup, missing source links, bootstrap guidance that can omit skills, or a
  failed GitHub Pages build.
- **Reversal:** remove only `docs/skills/index.html`, reverse the catalog CSS
  and links, and restore the prior optional bootstrap wording; if published,
  use a revert commit.
- **State safety:** do not remove canonical skill bundles, user-installed
  skills, unrelated docs, Git history, or runner configuration.
- **Recovery verification:** confirm all eight skills remain discoverable,
  the previous homepage loads, repository links resolve, and Pages returns to
  the baseline commit.

## Acceptance Criteria

- [ ] The requested outcome is complete.
- [ ] Relevant control-plane files are updated.
- [ ] Verification is complete or clearly documented.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- List all eight skills with GitHub CLI and `npx skills`.
- Install all eight into an isolated project.
- Validate local Markdown links, HTML anchors/assets, and SVG parsing.
- Serve `docs/` locally and request `/skills/`.
- Run `git diff --check`.
- Confirm the final GitHub Pages build and live catalog.
