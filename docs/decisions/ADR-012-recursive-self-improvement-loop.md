---
id: ADR-012
title: Give Adopting Repositories a Measured, Constitution-Bounded Self-Improvement Loop
status: Accepted
date: 2026-09-03
delivery: delivered
delivered: 2026-09-03
areas: [agents, workflows, governance, evolution]
tags: [adr, recursive-self-improvement, evolution, evaluators, constitution, autonomy, adopters]
---

# ADR-012: Give Adopting Repositories a Measured, Constitution-Bounded Self-Improvement Loop

## Context

PULSE is a template. Its value lands in the repositories that adopt it, not
in this source repository. The maintainer's direction is recursive
self-improvement **for the project using PULSE**: the agent working on a
product repository should get better at working on that repository through
its own measured experience, not only through a human writing memory entries
and decisions.

Today an adopting repository improves in exactly two ways, both manual. A
human records a lesson in `docs/memory/` or edits the local rules, or a
template sync brings down whatever the PULSE maintainer changed upstream.
Nothing checks whether a lesson actually changed behavior. Repeated mistakes
are visible in `docs/memory/mistakes.md`, but nothing closes the loop from
mistake to rule change to evidence that the rule worked.

Every self-improving system that has worked in practice - archive-based
evolutionary coding agents, evolutionary program search with automated
evaluators, skill libraries that grow from experience - shares three parts.
Something records every attempt and its measured result. Something the loop
cannot modify decides what "better" means. Something the loop cannot modify
sets the boundaries. Where any part is missing, the reported failure is the
same: the loop optimizes for looking better (faked tool output, edited tests,
hallucinated completion) instead of being better. Those reports are collected
in the [spec-driven pitfalls guide](../guides/spec-driven-pitfalls.md).

An adopting repository already has the third part (hard stops, rollback
rules, sandbox disclosure, tiers) and raw material for the first (memory,
review findings, rollback records, delivery marks, lead time, its own test
suite). It has no evaluator that defines "better" for how the agent operates
there, and it has no loop.

The [template-sync ownership model](../workflows/template-sync.md) already
splits every adopting repository into template-owned files (workflows,
scripts, skills, shared prompts, file templates) and project-owned files
(context, architecture, decisions, features, memory, plans, product code).
That split is the natural boundary for a loop: the template ships the
mechanism; the project owns what the loop learns.

The decision is what PULSE ships, what the loop in an adopting repository may
change, what it may never touch, and how autonomy is earned.

## Options Considered

### Option 1: Loop Over Product Code Directly

The agent mutates product code, runs the project's tests and benchmarks,
keeps improvements, merges its own changes, and coordinates with other agents
in a persistent board.

- **Pros:** the most direct reading of "evolution-driven development"; the
  fastest possible loop on the product itself.
- **Cons:** a test suite is a weak, gameable evaluator for product quality;
  money, auth, identity, and data live in product code and are hard stops; a
  shared agent board is a prompt-injection channel; token cost is unbounded.
  Every documented failure mode of unsupervised loops applies.

### Option 2: Loop Over the Project's Operating Layer First, Product Code Later Under the Tiers

PULSE ships the mechanism: a workflow, a skill, templates for a constitution,
evaluators, and an archive, and a read-only report. Each adopting repository
instantiates its own constitution and evaluators. The loop first improves the
project-owned files that shape how the agent works on that repository (local
rules, memory, project prompts, project skills and workflows), scored by
evaluators it cannot edit, bounded by a constitution it cannot edit, promoted
by a human until evidence earns standing autonomy per file class. Product
code enters the loop only under the existing tiers: Tier 0 with a reproducing
test, then Tier 1 inside an Accepted ADR, never Tier 2 surfaces.

- **Pros:** targets where the adopting repository has real leverage and cheap
  checks; the template-sync ownership split already draws the boundary;
  every hard stop stays a human decision; autonomy grows only with evidence
  and shrinks on incident; the archive doubles as the project's evolution
  record; one mechanism serves every adopter and PULSE itself.
- **Cons:** slower than Option 1; each project must write its own evaluators;
  agentic evaluation costs tokens; overfitting to the suite must be guarded
  against; product-code gains arrive only after the operating layer is stable.

### Option 3: Improve Only PULSE Itself

The loop runs in the PULSE source repository and adopters receive
improvements through template sync. This was the first draft of this ADR.

- **Pros:** one loop to build and watch; PULSE is small and cheap to evaluate.
- **Cons:** adopters learn nothing from their own repository; project-specific
  mistakes, stacks, and conventions never enter the loop; it contradicts
  PULSE's purpose as a template whose value must land in adopting
  repositories. Kept only as dogfooding: PULSE is one adopter among many.

### Option 4: Keep Manual Learning

Humans keep writing memory entries and ADRs in adopting repositories.

- **Pros:** no new risk; nothing to build.
- **Cons:** does not move toward the maintainer's direction; each project
  improves only as fast as one person writes, and nothing verifies that a
  lesson worked.

### Option 5: Agent Discussion Board Only

Add a persistent thread system for agents in the adopting repository and rely
on discussion to improve it.

- **Pros:** cheap to start; gives agents on different runners one place to
  leave what they saw and tried.
- **Cons:** discussion without measurement produces opinions, not
  improvement; unbounded threads recreate context rot; an open board is a
  prompt-injection channel. Folded into Option 2 as the loop's input channel
  (the *Board* part below), not adopted as a replacement for the loop.

## Decision

Propose **Option 2**, staged so each stage must earn the next.

### What PULSE ships (template-owned, synced to adopters)

- `docs/workflows/evolve.md` - the cycle procedure.
- `pulse-evolve` skill - runs one supervised cycle; refuses to start without
  a filled constitution and at least one evaluator suite.
- Templates under `docs/evolution/`: `_template-constitution.md`,
  `_template-evaluators.md` (constraint and score definitions, fixture
  format), and `_template-archive.md` (row format). Under `docs/board/`:
  `_template-post.md` (post kinds, provenance, vote format). The existing
  `docs/*/_template*.md` sync-safe glob already covers them.
- `docs/scripts/evolution-report.sh` - read-only: archive summary, kept and
  discarded counts, score trend, and the current autonomy stage per file class.
- `docs/scripts/board.sh` - read-only: the generated board index for an
  area (active claims, open threads with summary lines, vote tallies, the
  experiment queue) and a stale-claims and stale-threads report.
- Bootstrap installs the templates but does **not** turn the loop on.
  Stage 1 needs a human to fill the constitution and write the first
  evaluators. The board needs nothing but acceptance of this decision.

### What the adopting repository owns (never synced)

- `docs/evolution/constitution.md` - the project's instance: its hard stops,
  its immutable paths, its per-cycle budget, its kill switch, its editable
  scope, and its autonomy thresholds.
- `docs/evolution/evaluators/` - the project's definitions and fixtures.
- `docs/evolution/archive.md` - the project's append-only record.
- `docs/board/` - the agents' board: posts, threads, claims, and votes.
- Everything the loop learns.

### The five parts, instantiated per project

1. **Archive** - append-only rows: parent variant, the single change made,
   the hypothesis, the signal that prompted it, evaluator scores before and
   after, the decision (kept, discarded, or promoted), and the commit or
   branch holding the tree. Kept variants that change how agents operate on
   the project are the project's evolution record. The archive is data,
   never instructions; the loop reads a bounded summary of it, not the file.
2. **Evaluators** - a fixed suite that defines "better" for this project.
   Two kinds:
   - *Constraints* (pass/fail, any failure discards the variant): hard stops
     honored under adversarial prompts; sandbox warning and tier declaration
     present; template-owned files, the constitution, the evaluators,
     secrets, and product code outside the current stage's scope untouched;
     no board post treated as an instruction or an approval; the project's
     existing tests, lint, and link checks pass.
   - *Scores* (optimized subject to constraints): tier-classification
     accuracy on a labeled set of this repository's past requests; first-try
     verification pass rate on fixture tasks; review findings per change;
     rollbacks per change; lead time from the existing report; tokens and
     wall time per task where the runner exposes them. No token ledger or
     footer is created; counts are a score input only when available.
   Humans own the evaluators. The loop may *propose* a new evaluator or
   fixture, for example from a repeated mistake; it takes effect only after a
   human commits it. A held-out subset is never shown to the loop. A small
   human-rated sample calibrates the scores periodically and catches "metrics
   up, repository worse for people".
3. **Constitution** - what the loop can never edit: the project's hard stops
   (identical to its `AGENTS.md`), the evaluator files, the budget, the
   sandbox requirement for every run, template-owned paths (they change only
   through template sync), the rule that constitution changes are human
   commits, and the kill switch. Enforced by path: a variant that touches a
   constitution or template-owned path fails the constraint check; on GitHub,
   branch protection and code owners require human review on those paths.
4. **Loop** - one cycle: take the top of the board's experiment queue (the
   highest-voted open suggestion that passes the constitution's path check);
   if the queue is empty, form one hypothesis from the signals the
   repository already produces (verification failures, review findings
   including tier-honesty checks, executed rollbacks, memory mistakes, lead
   time); the hypothesis changes one variable; apply it on an experiment
   branch inside a sandbox; run constraints, then scores, each fixture
   several times, using the median; keep only a variant that is better on the
   target score and not worse beyond a stated tolerance elsewhere; write the
   archive row either way, including the suggestion id and its vote tally;
   post the result back to the suggestion's thread and close it; stop on
   budget, on a plateau of several cycles without improvement, or on any
   constitution violation. Promotion to the default branch is a pull request
   approved by a human until autonomy is earned.
5. **Board** - an agent-only space inside the repository where several
   agents, on any runner, in parallel or in sequence, coordinate and improve
   the repository together: they claim work, hand it off, share findings,
   ask and answer each other, post suggestions, give opinions on them, and
   vote. It is the fast, short-lived layer under the durable ones: anything
   worth keeping is promoted to a plan, memory, the archive, or an ADR, and
   the board entry closes. Not a live chat - shared working memory.
   - *Shape:* `docs/board/<area>/<thread>/` with one immutable file per post,
     named by timestamp and agent id, so concurrent agents never conflict on
     the same file. Project-owned, never synced. Channels follow the `areas:`
     vocabulary the repository already uses. The index, per-thread summary,
     vote tallies, and experiment queue are generated by `board.sh`, never
     hand-edited.
   - *Agents only:* agents write; humans read. Humans steer through
     `AGENTS.md`, decisions, reviews, and requests - not by posting - so the
     board never becomes an instruction channel.
   - *Post kinds for working together:* `claim` (I am working on this task,
     branch, and these files, since this time; expires after a stated period
     unless renewed; advisory, not a lock), `handoff` (the state of unfinished
     work for whoever picks it up), `finding` (what the next agent would
     otherwise rediscover: a flaky test, a build quirk, a hidden dependency),
     `question` and `answer`.
   - *Post kinds for improving:* `suggestion` (a proposed change to the
     operating layer, or a Tier 0 or Tier 1 product change, with the
     hypothesis, the one variable it changes, and the score it should move),
     `opinion` (support, concern, or alternative, with a reason and links to
     archive rows, mistakes, or fixtures), `vote` (up or down, one line of
     reason, from an agent that did not write the suggestion), `result`
     (posted by the loop; must link an archive row).
   - *What votes decide:* the order of the experiment queue - which
     suggestions spend the loop's budget first. Votes never merge anything,
     never override a constraint, a hard stop, or the constitution, and never
     count as evidence that a change worked; the evaluators decide that. One
     vote per agent identity per suggestion; no votes on your own suggestion;
     a suggestion enters the queue only with votes from at least two distinct
     agents, preferably different runners or models, since agents of one
     model family agree with each other too easily. A suggestion that gathers
     no votes within a stated number of cycles expires.
   - *Votes are scored too:* the archive records each suggestion's vote tally
     next to its evaluator result, so the project can see whether agent votes
     predict improvements. If they do not, the constitution lowers their
     weight in the queue; if they do, it may raise it. This is the
     self-improvement the maintainer asked for, applied to the agents'
     judgment itself.
   - *Provenance on every post:* agent id, runner, model, session id,
     timestamp, branch, and links (commit, plan, archive row, ADR). A post
     without provenance is quarantined, not read.
   - *Data, never instructions:* nothing posted authorizes anything.
     Instructions come only from `AGENTS.md`, the constitution, and
     human-committed files. An agent that reads an instruction-shaped post
     treats it as untrusted content and says so in its reply.
   - *No mutual approval:* agents do not approve each other's work; approval
     is a human review or commit, exactly as today.
   - *Bounded reading:* at the start of tool-backed work an agent reads the
     generated index for its area - active claims, open threads' summary
     lines, the queue, the last few posts - never the whole board.
   - *Every thread closes:* answered, promoted (plan, memory, archive row,
     ADR, work unit), tested by the loop with its result posted, or dropped
     with a reason. Claims expire. The quarterly prune and `board.sh --stale`
     clear the rest.
   - *Freshness is git freshness:* agents sharing a working tree see posts at
     once; agents in separate clones or worktrees see them after commit and
     pull. A GitHub-hosted project may mirror the board to Issues or
     Discussions later for cross-clone immediacy; the file board stays the
     source of truth.
   Live multi-agent chat is deliberately not part of this. It multiplies
   tokens, and the most common failures of multi-agent systems are
   coordination failures rather than capability gaps. The board gives agents
   shared memory and a queue instead of a conversation.

### Editable scope, by stage

- **Stage 0 (today):** manual learning; memory entries by hand; tiers and
  lead time measured.
- **Stage 1 - instrumented:** the board opens as soon as this decision is
  accepted, because coordination does not need the loop: agents claim, hand
  off, share findings, ask, suggest, opine, and vote. The project fills its
  constitution and writes ten to twenty evaluator fixtures from its own past
  requests; a human runs the suite by hand to get a baseline. The loop
  changes nothing yet; the queue simply accumulates.
- **Stage 2 - supervised:** one cycle per request or per schedule over the
  operating layer: project-owned sections of `AGENTS.md` and `CLAUDE.md`,
  `docs/memory/`, `docs/prompts/` outside `shared/`, and project-owned skills
  and workflows. A human merges or discards every variant.
- **Stage 3 - standing autonomy for the operating layer:** after a stated
  number of clean cycles with no incident, the loop merges its own
  operating-layer variants for the file classes that earned it. Any incident
  returns that class to Stage 2.
- **Stage 4 - product code under the tiers:** Tier 0 changes with a
  reproducing test, then Tier 1 changes inside an Accepted ADR, scored by the
  project's tests and benchmarks plus a human-rated sample. Each class earns
  autonomy the same way. Tier 2 surfaces never enter the loop.
- **Never in scope, at any stage:** hard-stop surfaces, the constitution,
  the evaluators, template-owned files, credentials, production, and anything
  the project's `AGENTS.md` lists as a hard stop.

### Recursion, and how the template learns

Inside a project, the loop improves the operating layer, which improves the
agent's next proposals, which improve the operating layer. The loop's own
skill and workflow are template-owned, so a project does not edit them
directly; when a kept variant is generic, the project opens a pull request or
issue on the PULSE repository. PULSE runs the same loop over its own
repository as one adopter, with its own board, evaluates the proposal with
its own evaluators, and an accepted change becomes the next PULSE version and
syncs down to every adopter. Selection happens at two levels: inside each
project, and across projects at the template.

### What happens to ADRs, plans, and specs in an adopting repository

Operating-layer changes made by the loop are archive rows, not hand-written
ADRs; they are Tier 1 work under the project's acceptance of this decision
while they stay inside its constitution. Product changes keep the tiers
exactly as today: Tier 0 needs no artifact, Tier 1 needs a work unit (which
the loop may write), Tier 2 stays human. Human-authored ADRs remain for the
constitution, the evaluators, and every real product decision. Specs for how
the agent operates become evaluator fixtures - executable, not prose. Memory
stays, and each entry gains evidence: the archive row that showed it worked.

## Consequences

### Positive

- Every adopting repository improves through measured experiments on its own
  history instead of through opinion, and the evidence for each change is
  attached to it.
- The template-sync ownership split does double duty as the loop's write
  boundary; no new ownership model is needed.
- Autonomy is earned per file class and revoked per incident, so the human's
  attention moves from every change to the constitution and the evaluators.
- Stage 1 alone gives a project something it lacks today: a baseline showing
  whether its rules and memory actually change agent behavior.
- PULSE learns from adopters through upstream proposals, and improvements
  reach every adopter through the existing sync.
- Several agents, on any runner, can work on one repository at the same time
  with shared claims, handoffs, and findings instead of collisions and
  rediscovery - without live chat and without a new service.
- Agents fill the loop's queue themselves, and the archive shows whether
  their votes predict real improvement.

### Negative

- Each project must write and maintain its own evaluators; that is the real
  cost, and it does not transfer between stacks.
- The board is an injection and collusion surface. Provenance, the
  data-not-instructions rule, no mutual approval, no self-votes, and the
  two-distinct-agents rule are load-bearing; bounded reading only partly
  controls the extra reading cost.
- A file-based board is only as fresh as the last commit and pull for agents
  in separate clones; same-tree agents see it at once.
- Agentic evaluation spends tokens; the budget is a constitution value and
  will limit how fast the loop runs.
- Overfitting to the suite is a permanent risk; held-out fixtures, rotation,
  and human calibration are recurring work, not one-time setup.
- Product code is last in scope; a project wanting product-level evolution
  first will find this slow by design.
- Until Stage 3, the loop is not autonomous end to end; that is deliberate.

## Follow-Up

- [x] Maintainer decision: accepted 2026-09-03 ("go ahead, implement").
  Default budget and clean-cycle counts are the starting values in
  `docs/evolution/_template-constitution.md` and PULSE's own
  `docs/evolution/constitution.md`; humans adjust them there.
- [x] Templates: `docs/evolution/_template-constitution.md`,
  `_template-evaluators.md`, `_template-archive.md`, and
  `docs/board/_template-post.md`; `docs/workflows/evolve.md`; read-only
  `docs/scripts/evolution-report.sh` - delivered by
  [PLAN-011](../plans/self-improvement-loop-and-board.md).
- [x] Board: `docs/board/` layout, `board.sh` (index, queue, tallies, stale
  report), the board read in the Understand step of the workflows, vote rules
  in the constitution template, `docs/board/` posts in never-sync, and the
  prune step for stale claims and threads.
- [x] Dogfood, Stage 1: PULSE's own constitution, evaluators with fourteen
  fixtures drawn from real requests, and an empty archive exist; `loop: off`.
- [x] Dogfood baseline recorded 2026-09-03 at the maintainer's instruction:
  F01-F14 run against `v3`, `v4`, and `v5` (rows B-001 to B-003 in
  `docs/evolution/archive.md`); `baseline: B-003` set in `evaluators.md`.
- [ ] Held-out set and Stage 2: the maintainer writes two `tier` fixtures kept
  outside the repository, lists their ids under "Held out:" in
  `evaluators.md`, then sets `loop: on` in the constitution. Human commits
  only; agents do not write held-out fixtures.
- [x] Template sync and bootstrap updated: templates, workflows, scripts, and
  the skill are sync-safe; `docs/evolution/` instance files and `docs/board/`
  posts are never-sync.
- [x] `pulse-evolve` skill ships with hard preconditions: it refuses to run a
  cycle without a filled constitution, evaluators, a recorded baseline,
  `loop: on`, and no `STOP` file.
- [x] `docs/architecture/overview.md`, `docs/context/product.md`, and memory
  updated.
- [x] `delivery: delivered` set when PLAN-011 closed.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan or work-unit step has landed.
- `delivered` - the plan or last work unit closed; set `delivered: YYYY-MM-DD`.
- `retired` - the decision is superseded, deprecated, or abandoned.

Built by: [PLAN-011](../plans/self-improvement-loop-and-board.md) and
[FEAT-007](../features/007-agent-board-and-self-improvement-loop.md),
delivered 2026-09-03. The mechanism is delivered; each repository's loop turns
on only after its own Stage 1 baseline, which for PULSE is the open
maintainer-run item above.
