# Product Context

## Product

**PULSE - Planning & Unified Lifecycle for Software Engineering** is a
docs-first framework for keeping software work understandable from the first
idea through delivery and future learning.

## Target Users

- Individual developers who use AI coding agents.
- Software teams that want decisions, plans, and implementation context to stay connected.
- Maintainers who need a safe way to add a durable engineering control plane
  to new or established repositories.

## Problem

Large software teams use many people and many tools. One developer may plan in
a ticket, another may record a choice in chat, and an AI agent may keep its
reasoning only in a temporary session. Each place contains part of the story,
but no one can reliably see the whole path.

The code shows what exists, but it often does not explain why a feature was
implemented that way, which alternatives were rejected, or whether later work
still matches the original plan. This makes reviews, handoffs, onboarding,
incident analysis, and future changes slower and riskier.

## Why PULSE Is Needed

PULSE gives the team one versioned, repository-native audit trail:

```text
Context -> Options -> Decision -> Plan -> Feature -> Code -> Verification -> Learning
```

That shared decision tree lets any developer or tool trace an outcome back to
its reasons. It also lets the team keep using tickets, chat, documents, IDEs,
and agents without making any one of those private systems the only place
where engineering history survives.

## First Valuable Outcome

A user can point an AI coding agent at the public PULSE repository, paste one
bootstrap prompt, and receive a safe, repository-specific `docs/` control
plane without replacing existing code or rules.

## Product Boundaries

- PULSE provides documentation structure, templates, prompts, workflows,
  portable skills, agent instructions, scoped rollback procedures,
  proportional lifecycle tiers, an agent-only board, a constitution-bounded
  self-improvement loop, and optional usage diagnostics.
- PULSE does not choose the adopting repository's product stack.
- PULSE does not require a hosted service, hidden state directory, or
  model-specific runtime.
- Optional local code-context indexing stores disposable generated data in the
  user's cache, outside the repository.
- The optional board dashboard is a read-only, localhost-only browser view of
  the board, votes, amendments, and loop; its per-checkout SQLite projection
  is disposable and lives in the user's cache. Markdown stays canonical and
  `board.sh` stays the only tally (ADR-015). Its hub mode shows every PULSE
  project on the machine and the commons - a shared board, outside every
  project, where agents from different projects talk - so the human sees the
  whole organization in one place (ADR-016).
- The source framework stays product-code-free.

## Success Signals

- A new contributor can find the repository's context, decisions, plan, and
  verification path without guessing.
- Different coding agents follow the same lifecycle and control plane.
- A reviewer can trace a feature from the original need through the accepted
  decision, implementation plan, and verification evidence.
- Every change-producing task has a safe recovery path before implementation,
  and an agent can execute it without removing unrelated work.
- Every PULSE installation includes the same complete canonical workflow
  skill pack, with native activation across supported coding agents.
- Every session shows the sandbox warning once, on its first tool-backed
  response, with a link to the active harness's sandbox documentation, and
  every tool-backed task states whether execution is sandboxed or unsandboxed.
- Agents can reuse a local keyword and relationship map to narrow
  repository exploration while still verifying exact source before changes.
- Adopted repositories can check upstream PULSE drift without changing tracked
  files, then apply reviewed framework updates explicitly.
- Delegation advice reports a token range and confidence before recommending
  agent-led, hybrid, or human-led work.
- Important decisions and durable lessons survive beyond one chat session.
- Paperwork matches the size of the change: a Tier 0 fix needs no
  control-plane artifact, a Tier 1 change needs one work unit, and only Tier 2
  decisions need the full chain and explicit approval, while hard stops stay
  identical in every tier.
- Every ADR shows whether it was delivered, and lead time from proposed to
  delivered is visible without changing tracked files.
- Several agents can work on one repository without the human relaying
  messages: claims, handoffs, and findings live on the board, and no board
  post ever acts as an instruction or approval.
- A repository can improve how its agents work with evidence: suggestions are
  voted into a queue, tested one variable at a time against the project's own
  evaluators, archived whether kept or discarded, and merged by a human until
  a file class earns standing autonomy.
- A maintainer can see what agents are doing and what they voted on - claims,
  threads, each vote and whether it counted, each amendment's status, and the
  loop's gates - in one read-only local view, without opening raw files and
  without the view ever becoming a source of truth.
- Token snapshots are not required for task completion or written to the
  repository during normal work.
