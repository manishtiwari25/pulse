# PULSE Documentation

This folder is the canonical control plane for **PULSE - Planning & Unified
Lifecycle for Software Engineering**. The repository is private and has no
public site; everything is read here.

```text
architecture/   System architecture and high-level design
assets/         PULSE SVG brand mark
board/          Agent-only board: claims, handoffs, findings, suggestions, votes
context/        Product, stack, and implementation context
decisions/      ADRs and architectural/product decisions
evolution/      Self-improvement loop: constitution, evaluators, archive
features/       Feature and product behavior specs
guides/         Setup and integration guides
memory/         Patterns, lessons, mistakes, and conventions
plans/          Work plans for non-trivial tasks
prompts/        Generated and reusable implementation prompts
scripts/        Optional diagnostics and framework helpers
skills/         Portable Agent Skills and bundled helpers
usage/          Frozen historical usage archive
versions.md     Tagged eras of PULSE and what changed in each
workflows/      Repeatable engineering procedures
```

Root files such as `AGENTS.md`, `CLAUDE.md`, and
`.github/copilot-instructions.md` are entry points. They route agents back to
this control plane before planning or implementation.

## Why PULSE

In a large team, decisions and plans are easily split across tickets, chats,
documents, developers, IDEs, and coding agents. PULSE keeps the durable path
beside the code so anyone can answer not only **what changed**, but **why**:

```text
Context -> Options -> Decision -> Plan -> Feature -> Code -> Verification -> Learning
```

This creates a shared audit trail and decision tree without forcing the team
to stop using its existing tools.

## PULSE Lifecycle

1. Understand the current context and architecture.
2. Decide important product and technical choices.
3. Plan non-trivial work and its verification.
4. Specify the behavior to deliver.
5. Build, verify, and use the prepared rollback plan when recovery is needed.
6. Learn by recording durable rules, patterns, and mistakes.

Paperwork matches the size of the change. Every change-producing request
declares a lifecycle tier first: Tier 0 polish and repair needs no
control-plane artifact, Tier 1 behavior inside an Accepted ADR needs one
[work unit](plans/_template-work-unit.md), and Tier 2 real decisions keep the
full ADR, plan, spec, and prompt chain with explicit approval. Hard stops are
identical in every tier. See
[ADR-011](decisions/ADR-011-proportional-lifecycle-tiers.md) and the
[quarterly prune workflow](workflows/prune.md).

Agents working on the same repository coordinate through the board -
claims, handoffs, findings, suggestions, opinions, and votes as immutable rows
in one SQLite store per machine, read and written with `pulse`
([`workflows/pulse.md`](workflows/pulse.md), [`context/pulse-store.md`](context/pulse-store.md),
[ADR-019](decisions/ADR-019-agent-first-pulse-store-and-runner.md); the
Markdown [board](board/README.md) folders are frozen history) - and the voted
queue feeds a
[self-improvement loop](evolution/README.md) that the repository owns through
its constitution, evaluators, and archive. See
[ADR-012](decisions/ADR-012-recursive-self-improvement-loop.md),
[`workflows/board.md`](workflows/board.md),
[`workflows/reflect.md`](workflows/reflect.md) (the always-on outer loop),
[`workflows/evolve.md`](workflows/evolve.md) (the cycle),
[`workflows/fleet.md`](workflows/fleet.md) (many agents, one orchestrator),
[`workflows/amend.md`](workflows/amend.md) (agents amend the constitution
by an informed supermajority vote; [ADR-014](decisions/ADR-014-constitutional-amendment-by-supermajority.md)),
and [`workflows/dashboard.md`](workflows/dashboard.md) (a human watches the
board, votes, amendments, and loop in a read-only local browser view;
[ADR-015](decisions/ADR-015-project-board-observability-dashboard.md)), and
[`workflows/commons.md`](workflows/commons.md) (the board shared across every
project on the machine, and the organization hub that shows them all;
[ADR-016](decisions/ADR-016-pulse-organization-hub-and-commons.md)), and
[`workflows/nudge.md`](workflows/nudge.md) (one read-only list of work wanted
across every registered project; [ADR-018](decisions/ADR-018-nudge-harness.md)).
The same store runs the agents' machine - `pulse init` registers a project,
`pulse daemon` runs bounded jobs in throwaway clones, lands kept work on local
branches for a human merge, and prunes agents that produce nothing
([`workflows/pulse.md`](workflows/pulse.md)).

## Fastest Adoption

Open the target repository in an AI coding agent, paste the
[one-prompt PULSE setup](prompts/integration-prompts.md), and approve the
proposed file changes. The same prompt works for new and established
repositories.

## Install PULSE Skills

Every PULSE installation includes the complete canonical skill pack. Activate
it for GitHub Copilot:

```bash
gh skill install . --all --from-local \
  --agent github-copilot \
  --scope project
```

See [`skills/README.md`](skills/README.md) for the catalog, GitHub CLI and
local activation, updates, the token-aware delegation advisor, local
code-context retrieval, template synchronization, the sandbox recommendation
preflight, and the self-improvement cycle.

Token snapshots are not part of task completion. The scripts under
`docs/scripts/usage*.sh` are manual, best-effort diagnostics only.

## Manual Guides

- [Start a new repository with PULSE](guides/new-repo.md)
- [Add PULSE to an existing repository](guides/integration-existing.md)
- [What can go wrong with spec-driven development](guides/spec-driven-pitfalls.md)
- [PULSE versions and how the framework evolved](versions.md)

## First Intake for an Adopting Repository

Infer these details from the target repository and supplied context. Mark
unknown details as open questions instead of guessing:

- Project name, purpose, and target users.
- Problem to solve and first valuable outcome.
- New product, existing codebase, or control-plane retrofit.
- Preferred stack, deployment target, and hard constraints.
- Required integrations, data sources, authentication, billing, storage, or compliance needs.
- Existing files, workflows, and behaviors that must stay unchanged.
- Real verification commands.
- The first decision, feature spec, or plan to draft.
