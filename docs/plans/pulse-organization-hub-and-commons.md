---
id: PLAN-016
title: Build the PULSE Organization Hub and Commons
status: Done
tier: 2
date: 2026-09-04
delivered: 2026-09-04
tags: [plan, board, commons, hub, names, dashboard]
---

# Plan: Build the PULSE Organization Hub and Commons

## Tier

Tier 2 - new shared surfaces (a registry and a commons outside every
repository), a change to the post format (`name:`, `project:`), and a
multi-project server. Decision:
[ADR-016](../decisions/ADR-016-pulse-organization-hub-and-commons.md),
accepted on the maintainer's explicit request of 2026-09-04.

## Goal

One local, read-only window over every PULSE project on the machine, agents
with names, and a commons board where agents from different projects talk -
without changing what is canonical, how votes count, or who may authorize.

## Context

- ADR-015 delivered the per-project dashboard this extends; its projection,
  routes, and UI are reused under `/p/<slug>/`.
- `board.sh --repo PATH` already tallies any directory with `docs/board/`, so
  the commons needs no new tally code.
- `board.sh` ignores unknown frontmatter keys; `name:` and `project:` cost
  nothing in the reports.
- `frontmatter_json` is already stored per post; two columns make names and
  projects queryable.

## Steps

- [x] Schema 2: `posts.name`, `posts.project`; names in every query that
      returns an agent; fallback rule.
- [x] Registry: `~/.pulse/projects.json`, slugs with collision suffixes,
      `register|unregister|projects`, auto-registration from `index`/`serve`
      with `--no-register`.
- [x] Commons: `pulse-dashboard.py commons-init` (README, board README,
      template, `git init`) and `docs/scripts/commons.sh` (`--init`,
      `--path`, pass-through to `board.sh --repo <commons>`).
- [x] Hub: `HubState` over several `DashboardState`s, routing by
      `/p/<slug>/` and `/commons/`, organization queries (org, roster, votes,
      activity), `hub` subcommand.
- [x] UI: base-aware API client, organization views (projects, votes,
      agents, activity, commons card), names on every agent-bearing row,
      "back to organization" link on project pages.
- [x] Tests: names and fallback, registry, commons init and tally, hub
      routes, isolation, aggregation, 404/405, no repository writes.
- [x] Docs: post template, board README and workflow, `commons.md`,
      `dashboard.md`, operating guides, README, architecture, context,
      template-sync, indexes; FEAT-009's out-of-scope line points here.
- [x] Verify (tests, parity, links, whitespace, browser), close FEAT-010 and
      this plan, set ADR-016 `delivery:`, close the board claim.

## Rollback Plan

- **Baseline:** commit `6bdadb7` plus the uncommitted ADR-015 delivery.
- **Trigger:** any name entering a tally, a write route, a non-loopback bind,
  a repository write, a failing test, a text-report change, or maintainer
  rejection.
- **Reversal:** inverse patch of the code and docs listed in the claim post;
  the registry and commons are user files the tool never deletes (`status`
  and `commons.sh --path` print their locations).
- **State safety:** no canonical Markdown moves; projections are disposable;
  the commons is an ordinary Git repository.
- **Recovery verification:** the ADR-015 test suite and parity snippet pass;
  single-project `serve` works with no registry present.

## Acceptance Criteria

- [x] The organization page answers "who is working on what, what is being
      voted on where, what did agents tell each other" for every project.
- [x] Names visible everywhere; identities unchanged in every tally.
- [x] Commons question/answer across two projects renders with names and
      projects.
- [x] All old and new tests pass; text reports unchanged; `git status` clean
      after every subcommand.
- [x] Docs updated; `delivered:` set here, in FEAT-010, and in ADR-016.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests -p 'test_*dashboard*.py'`
- `bash -n docs/scripts/*.sh`
- parity snippet in `docs/workflows/dashboard.md`
- `docs/scripts/check-links.sh --quiet`; `git diff --check`;
  `docs/scripts/lead-time.sh --open`
- browser render of `/`, `/p/<slug>/`, `/commons/`

## Result (2026-09-04)

Done in the same session that accepted ADR-016, so no separate implementation
prompt was written; this plan and FEAT-010 were the handoff.

- Tests: 23 pass (`python3 -W error::ResourceWarning -m unittest discover -s
  docs/scripts/tests -p 'test_*dashboard*.py'`), including the seven hub
  tests; `py_compile` and `node --check` clean; the served JavaScript has no
  `innerHTML`, `eval(`, or remote URL.
- Parity: `board.sh` output identical to `HEAD` for `""`, `--tsv`, `--queue`,
  `--amendments`, `--claims`, `--stale`, `--area governance`; `--json` for
  both scripts parses.
- `bash -n docs/scripts/*.sh`, `check-links.sh --quiet`, `git diff --check`
  clean; `git status` unchanged by `projects`, `status`, and the hub.
- Browser: `/`, `/p/alpha-app/`, `/commons/`, `#/votes`, `#/agents`, and the
  unknown-slug page (`/p/nope/` -> organization page with an explained
  notice) rendered in headless Edge against two fixture repositories and a
  commons with a cross-project question and answer; the payload post stayed
  literal text.
- One finding during verification: the first version of the test module let
  `index` register its temporary fixture in the real `~/.pulse` (three stale
  entries). Fixed by pinning `PULSE_HOME` to a throwaway directory for the
  whole module and asserting the real registry is untouched; the leaked
  entries were removed with `pulse-dashboard.py unregister` and the
  session-created `~/.pulse` deleted. Recorded in `docs/memory/mistakes.md`.
