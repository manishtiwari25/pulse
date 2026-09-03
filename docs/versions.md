# PULSE Versions

Git keeps every past state of PULSE. This page makes that history readable:
what the system was at each version, what changed, why, and what evidence
drove the change. Each version is a git tag, so the old system can be viewed
exactly as it was:

```bash
git checkout v3          # the last universal-gate version
git diff v3 v4 -- AGENTS.md
```

On GitHub, open `https://github.com/manishtiwari25/pulse/tree/vN`.

## Timeline

| Version | Span | Tag points at | Governing ADRs | One line |
| --- | --- | --- | --- | --- |
| [v1](#v1---project-brain) | 2026-05-23 to 2026-08-08 | `8acc5e6` | ADR-001, ADR-002 | Docs control plane, one universal gate, mandatory token accounting |
| [v2](#v2---pulse) | 2026-08-11 to 2026-08-14 | `e474ee2` | ADR-003 to ADR-006 | PULSE brand, public site, agent-driven rollbacks, portable skills |
| [v3](#v3---sandbox-aware-and-optional-telemetry) | 2026-08-22 to 2026-08-28 | `50db38e` | ADR-007 to ADR-010 | Local code context, sandbox recommendation, telemetry made optional |
| [v4](#v4---proportional-lifecycle-tiers) | 2026-09-03 onward | tag after commit | ADR-011 | Three declared tiers, delivery marks, quarterly prune, lead time |
| v5 (proposed) | - | - | ADR-012 (Proposed) | Self-improvement loop for adopting repositories: archive, evaluators, constitution, agent-only board with suggestions and votes, staged autonomy |

Retired along the way: the mandatory token footer and usage ledger (ADR-002,
retired by ADR-010) and fail-closed sandboxing (ADR-008, retired by ADR-009).

## v1 - Project Brain

**Span:** 2026-05-23 to 2026-08-08 (`d69ecfb` to `8acc5e6`). The repository
was then called `my-coding-setup` and the framework "project brain".

**What the system was**

- A docs-first control plane under `docs/` with context, architecture,
  decisions, features, plans, prompts, workflows, and memory
  ([ADR-001](decisions/ADR-001-docs-control-plane.md)).
- One universal gate: every change needed an ADR, a plan, a feature spec, and
  an implementation prompt, plus explicit approval before implementation.
- Mandatory work accounting: a token footer on every response and a usage
  ledger in the repository
  ([ADR-002](decisions/ADR-002-work-accounting-token-reporting.md)).
- A template-sync workflow, ELI5 as the default output format, and the first
  bootstrap guides for new and existing repositories.

**Why it changed:** the framework was generic enough to publish, and the
control plane had no brand, no public site, and no recovery rule.

## v2 - PULSE

**Span:** 2026-08-11 to 2026-08-14 (`4159336` to `e474ee2`).

**What changed**

- Renamed to PULSE with a static GitHub Pages site
  ([ADR-003](decisions/ADR-003-pulse-brand-and-public-pages.md)).
- Every change-producing task must prepare a scoped rollback plan before the
  first edit, and agents may reverse only their own isolated work
  ([ADR-004](decisions/ADR-004-agent-driven-rollbacks.md)).
- The workflows became portable Agent Skills sourced from `docs/skills/`, with
  a public catalog ([ADR-005](decisions/ADR-005-portable-pulse-skills.md)).
- A token-aware delegation advisor with optional user-supplied history
  ([ADR-006](decisions/ADR-006-token-aware-delegation-advice.md)).

**What stayed:** the universal gate and mandatory token accounting.

**Why it changed:** agents read too much source before acting, ran commands
with no isolation story, and the token footer had become a completion gate
rather than a diagnostic.

## v3 - Sandbox-Aware and Optional Telemetry

**Span:** 2026-08-22 to 2026-08-28 (`7360fb2` to `50db38e`). `v3` is the last
universal-gate version.

**What changed**

- Optional local code-context index and relationship graph, generated outside
  the repository ([ADR-007](decisions/ADR-007-local-code-context-index.md)).
- Fail-closed sandboxing was required
  ([ADR-008](decisions/ADR-008-sandbox-first-agent-execution.md)), then
  replaced five days later by a visible recommendation with honest
  unsandboxed disclosure
  ([ADR-009](decisions/ADR-009-recommend-sandboxes-and-disclose-unsandboxed-execution.md)).
  This is the first recorded self-correction: real use showed a hard gate
  blocked ordinary work without proving isolation.
- Usage telemetry became optional and the ledger was frozen
  ([ADR-010](decisions/ADR-010-optional-usage-telemetry.md)), retiring the
  mandatory accounting from v1.
- `pulse-template-sync` for checking upstream drift without modifying tracked
  files.

**What stayed:** the universal gate.

**Why it changed:** one gate for every size of change made the human the
bottleneck on small work, and nothing showed whether an accepted decision had
actually been built.

## v4 - Proportional Lifecycle Tiers

**Span:** 2026-09-03 onward. Tag `v4` when the change is committed.

**What changed**

- Three declared lifecycle tiers replace the universal gate: Tier 0 polish and
  repair needs no control-plane artifact, Tier 1 behavior inside an Accepted
  ADR needs one work unit, and Tier 2 real decisions keep the full chain with
  explicit approval. Hard stops are identical in every tier
  ([ADR-011](decisions/ADR-011-proportional-lifecycle-tiers.md),
  [FEAT-006](features/006-proportional-lifecycle-tiers.md)).
- A work-in-progress limit on Proposed ADRs per area.
- ADR delivery marks (`not started`, `partially delivered`, `delivered`,
  `retired`), a quarterly [prune workflow](workflows/prune.md), and the
  read-only `docs/scripts/lead-time.sh` report.
- A [spec-driven pitfalls guide](guides/spec-driven-pitfalls.md) that names
  the failure modes of docs-first work with published evidence.
- This versions page and tags `v1` to `v3`.

**What stayed:** the docs control plane, rollback plans, sandbox
recommendation, portable skills, and every hard stop.

**Proposed next:** [ADR-012](decisions/ADR-012-recursive-self-improvement-loop.md)
would give every repository that adopts PULSE a measured loop over its own
operating layer - a per-project archive, human-owned evaluators, an immutable
constitution, an agent-only board where agents coordinate, suggest, and vote
on what to try next, and autonomy earned per file class - with PULSE itself
running the same loop as one adopter. It becomes v5 only if accepted and
delivered; until then it is a Proposed ADR with `delivery: not started`.

## How Versions Work

- A version closes when an Accepted ADR changes how agents must operate - the
  gate, the artifacts, the autonomy, or the safety boundary. Tier 0 polish and
  Tier 1 work do not bump the version.
- The tag `vN` points at the last commit of that version and is annotated with
  its governing ADRs. Tags are created locally and pushed only by a maintainer
  (`git push origin --tags`), because publishing is a hard stop.
- Each version entry records four things: what the system was, what changed,
  why, and what was retired. The "why" links to the ADR; the evidence is the
  ADR's delivery mark and the lead-time report.
- The quarterly [prune pass](workflows/prune.md) checks that this page and the
  tags still match the ADR index.
- An adopting repository can find its PULSE version by comparing the
  `lastSyncedCommit` in `.template-sync` with these tags.

## Reading the Evolution

Each version is one variant of the framework. The ADR says what changed and
why; the delivery mark says whether it was built; the lead-time report says
how long it took; and the retired list says which ideas real use killed. Read
in order, the versions show PULSE getting smaller where it was heavy (token
footers, fail-closed sandboxes, one gate for everything) and firmer where it
mattered (rollback before change, hard stops, honest disclosure). Keep that
pattern: add structure only after a case proves the need, and retire it
visibly when it stops earning its keep.
