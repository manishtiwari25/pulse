---
id: WU-001
title: Make PULSE Versions Visible with Tags and a Versions Page
status: Done
tier: 1
date: 2026-09-03
delivered: 2026-09-03
controlling_adr: ADR-001
areas: [docs, governance]
tags: [work-unit, versions, evolution]
---

# Work Unit: Make PULSE Versions Visible with Tags and a Versions Page

The first Tier 1 work unit in PULSE. The maintainer asked to keep the old
system as versions so the evolution of PULSE stays visible. Git already holds
every past state; this work names the eras and explains them.

## Tier Declaration

- **Tier:** 1 - a documentation page and local git tags inside the existing
  docs control plane; no new rule, gate, dependency, or product code.
- **Controlling ADR:** [ADR-001: Use a Docs-Based PULSE Control Plane](../decisions/ADR-001-docs-control-plane.md)
- **Why this stays inside the ADR:** ADR-001 makes `docs/` the versioned home
  of framework knowledge. A versions page is framework knowledge, and tags are
  git's native way to name versions of that same tree.
- **Hard stops touched:** none. Tags are created locally only; pushing them is
  publication and stays with the maintainer.

## Outcome

Anyone can see what PULSE was at each version, what changed, why, and what
was retired, and can check out any earlier version by tag.

## Behavior

### User Story

As a PULSE maintainer or adopter, I want each era of the framework named and
explained so that I can read its evolution and inspect the old system exactly
as it was.

### Requirements

- [x] `docs/versions.md` lists every version with span, tag commit, governing
      ADRs, what the system was, what changed, why, and what was retired.
- [x] Annotated tags `v1`, `v2`, `v3` point at the last commit of each earlier
      era; `v4` is tagged when the tiers change is committed.
- [x] The page states the rule for when a version closes.
- [x] `README.md`, `docs/README.md`, `AGENTS.md`, and the public site link to
      the page.

### Interface Changes

Documentation and git tags only.

### Data Changes

None.

### Edge Cases

- A tag already exists with the same name: stop and report instead of moving
  it.
- History is rewritten upstream: tags keep pointing at the original commits;
  the prune pass reports the mismatch.

## Steps

- [x] Read the commit history and ADR dates to find the era boundaries.
- [x] Create annotated tags `v1` (`8acc5e6`), `v2` (`e474ee2`), `v3`
      (`50db38e`).
- [x] Write `docs/versions.md`.
- [x] Link it from `README.md`, `docs/README.md`, `AGENTS.md`, and
      `docs/index.html`; add it to the sync-safe paths in
      `docs/workflows/template-sync.md`; add a versions check to
      `docs/workflows/prune.md`.

## Implementation Prompt

- **Repository and boundary:** `manishtiwari25/pulse`; documentation under
  `docs/` and local git tags only.
- **Files likely to change:** `docs/versions.md` (new), `README.md`,
  `docs/README.md`, `AGENTS.md`, `docs/index.html`,
  `docs/workflows/template-sync.md`, `docs/workflows/prune.md`.
- **Patterns to reuse:** ADR index table style in `docs/decisions/README.md`;
  `doc-link` cards in `docs/index.html`.
- **Constraints:** do not move or delete existing tags; do not push; do not
  duplicate old rule text into new files - point at the tag instead.
- **Do not:** cross ADR-001's boundary, touch a hard stop, or remove unrelated
  user work.

## Rollback Plan

- **Baseline:** working tree at `50db38e` plus the uncommitted ADR-011 change
  set; no tags existed.
- **Trigger:** a broken link, a tag pointing at the wrong commit, or the page
  contradicting the ADR index.
- **Reversal:** `git tag -d v1 v2 v3`; delete `docs/versions.md` and this
  file; revert the link edits in the files listed above.
- **State safety:** no dependencies, generated files, deployments, or external
  state; tags are local until a maintainer pushes them.
- **Recovery verification:** `git tag -l` shows no `v*` tags; the Markdown
  link check passes; `git status --short` shows only the ADR-011 change set.

## Acceptance Criteria

- [x] The behavior above is delivered and verified.
- [x] The work stayed inside the controlling ADR's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- `git tag -n1` lists `v1`, `v2`, `v3` with their messages.
- `git rev-parse v1 v2 v3` resolves to `8acc5e6`, `e474ee2`, `50db38e`.
- Relative Markdown links in `docs/versions.md` resolve.
- `docs/index.html` parses with balanced tags.

## Closure

- [x] Set `status: Done` and `delivered: 2026-09-03` in the frontmatter.
- [x] ADR-001 already carries `delivery: delivered`; no change needed.
- [x] No new durable lesson beyond those recorded for ADR-011.
