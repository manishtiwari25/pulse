---
id: PLAN-001
title: Rebrand the Framework as PULSE and Publish Its Documentation
status: In Progress
date: 2026-08-11
tags: [plan, pulse, branding, github-pages, usage]
---

# Plan: Rebrand the Framework as PULSE and Publish Its Documentation

## Goal

Turn the generic docs-based control-plane framework into PULSE - Planning & Unified
Lifecycle for Software Engineering - with one public identity, a usable
GitHub Pages portal, and token-only work accounting.

## Context

- The repository is a public GitHub template.
- The `docs/` control plane remains the canonical framework structure.
- The public site must not add a package or build dependency.
- Existing setup links must point to the public PULSE repository.
- Usage reporting must remove price, credit, and billing fields.

## Steps

- [x] Define the PULSE product, lifecycle, architecture, and brand language.
- [ ] Update all public instructions, guides, prompts, and source links.
- [ ] Create the PULSE SVG and static GitHub Pages site.
- [ ] Change every usage collector and ledger rule to tokens only.
- [ ] Validate the site, links, SVG, scripts, and repository diff.
- [ ] Rename and publish the repository, then enable GitHub Pages.

## Acceptance Criteria

- [ ] PULSE is the visible framework identity across canonical surfaces.
- [ ] The public page loads from `https://manishtiwari25.github.io/pulse/`.
- [ ] The SVG is valid, original, accessible, and used by the site and README.
- [ ] No active usage instruction or helper reports billing information.
- [ ] Bootstrap links point to the renamed public repository.
- [ ] Verification is complete and the session token snapshot is recorded.

## Verification

```bash
git diff --check
bash -n docs/scripts/*.sh
xmllint --noout docs/assets/pulse-logo.svg
docs/scripts/usage.sh copilot <session-id>
```
