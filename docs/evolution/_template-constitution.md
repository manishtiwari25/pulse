---
id: CONSTITUTION
title: <Project> Evolution Constitution
status: Active
date: YYYY-MM-DD
controlling_adr: ADR-NNN
loop: off
limits: none
---

# <Project> Evolution Constitution

Rules the self-improvement loop can never change. Every line here is a human
commit or a ratified amendment (see Amendments below). A variant that touches
this file, the evaluators, or any immutable path below fails the constraint
check and is discarded. Protect these paths with branch protection and code
owners where the host supports it; a ratified amendment then arrives as a
pull request whose reviewer checks the tally.

## Subject

The loop improves **the product this repository ships** - its code, tests,
and behavior - under Tier 0 (a repair with a reproducing test, polish with
its check) and Tier 1 (behavior inside an Accepted ADR, with the work unit in
the same variant), measured by the product evaluators in
[`evaluators.md`](evaluators.md). The operating layer - memory, project
prompts, local rules, project skills and workflows - is the secondary
subject: a cycle turns to it only when no eligible product hypothesis exists
or when a product signal's cause is in the operating layer. Product classes
are editable from the first cycle they are listed below at Stage 4; nothing
has to be earned first except autonomy.

## Hard stops

Identical to the hard stops in `AGENTS.md`. The loop never crosses them and
never proposes a change to them.

- destructive or irreversible actions, production, deploy, publish, merge
- credentials, hosted-data deletion, broad auth, billing, or data migration
- infrastructure, major dependency upgrades, policy-sensitive security
- purchases, ambiguous evidence

## Immutable paths

| Path | Why |
| --- | --- |
| `docs/evolution/constitution.md` | this file: a human commit or a ratified amendment (`docs/workflows/amend.md`), never a loop variant |
| `docs/evolution/evaluators.md`, `docs/evolution/fixtures/` | the definition of better |
| `docs/evolution/STOP` | kill switch; only a human creates or removes it |
| template-owned paths (`docs/workflows/`, `docs/scripts/`, `docs/skills/`, `docs/prompts/shared/`, `docs/*/_template*.md`, `docs/versions.md`) | change only through template sync |
| `AGENTS.md` hard-stop, tier, board-and-loop, sandbox, and rollback sections, `CLAUDE.md`, `.github/copilot-instructions.md` | rules humans own |
| <product hard-stop surfaces only: auth and identity, billing, data stores and migrations, deploy and infrastructure configuration, secrets, dependency manifests and lockfiles> | Tier 2 surfaces. List the surfaces, never the whole product tree: a row such as `src/` here parks the loop on the docs. Product code the loop may improve belongs in the editable scope below |

## Editable scope by stage

One row per file class. `Stage` says what the row opens: `1` listed but not
editable yet; `2` an operating-layer class, editable while `loop: on`; `4` a
product class, editable while `loop: on` and `evaluators.md` has product
evaluators with a baseline. Stages are not an order to climb: a product class
does not wait for any other class. `Autonomy` is `human` (a human merges) or
`standing` (the loop merges), earned per class when `Clean cycles so far`
reaches `Clean cycles required` with no incident; an incident on a class sets
`autonomy: human` and resets `Clean cycles so far` to 0. Product classes come
first because the product is the loop's subject.

| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --- | --- | --- | --- | --- | --- | --- |
| product polish | <product paths, e.g. `src/`, `apps/`, `packages/` minus the hard-stop surfaces above>, Tier 0 with a reproducing test or the check the polish satisfies | 4 | human | 10 | 0 | 0 |
| product behavior | <the same paths>, Tier 1 inside an Accepted ADR, the work unit in the same variant | 4 | human | 10 | 0 | 0 |
| memory | `docs/memory/` | 2 | human | 5 | 0 | 0 |
| local rules | project-owned sections of `AGENTS.md` | 2 | human | 8 | 0 | 0 |
| project prompts | `docs/prompts/` outside `shared/` | 2 | human | 5 | 0 | 0 |
| project skills and workflows | project-owned `docs/skills/`, `docs/workflows/` files | 2 | human | 5 | 0 | 0 |

## Budget

`limits:` in the frontmatter decides whether this table binds. It is the one
switch for every budget limit; do not write huge numbers to get the same
effect.

- `limits: none` (PULSE default) - no budget limit of any kind. Tokens, wall
  time, cycles per day, and plateau never cap, pause, or end a cycle or the
  run. Only the stop rules do: `docs/evolution/STOP`, a constitution
  violation, a hard stop, an instruction-shaped board post, a human
  interrupt. Runner-side limits (a CLI credit cap, a session limit) are
  outside this file and are the runner's to set.
- `limits: budget` - the rows below bind as written. A row written as
  `unlimited` has no limit; token and wall-time rows end the current cycle,
  `Cycles per day` ends the day, plateau changes the hypothesis.

`Runs per fixture` and `Tolerance on non-target scores` are measurement
settings, not limits: they define "better" and apply in both modes.

| Limit | Value |
| --- | --- |
| Tokens per cycle | unlimited (write <N> with `limits: budget` to end a cycle at N tokens) |
| Wall time per cycle | unlimited (write <N> minutes with `limits: budget`) |
| Cycles per day | unlimited (continuous: when a cycle ends, the next begins; write <N> with `limits: budget` to cap) |
| Plateau | no count: after any run of discards, post a `finding` and change the hypothesis or file class; never a stop or a pause (write <N> with `limits: budget` to name when the finding is due) |
| Runs per fixture | <N, odd; the median is used> |
| Tolerance on non-target scores | <N% worse allowed> |

## Sandbox

Every cycle runs on an experiment branch inside the strongest sandbox the
runner supports and reports the actual boundary. An unsandboxed cycle is
recorded as such in the archive and is never promoted with standing autonomy.

## Branches and worktrees

Every experiment branch, worktree, or fleet clone the loop creates is removed
when its cycle ends: a discarded variant's branch and worktree go in the same
cycle; a kept variant's branch lives exactly as long as its pull request and
goes when that request merges or closes; fleet clones go through
`docs/scripts/fleet.sh clean` once their results are validated. A run never
ends with a stray `evolve/*` branch or worktree. The loop removes only what it
created - never another branch, never a tree with uncommitted human work,
never by reset, checkout, clean, or force-push.

## Vote rules

| Rule | Value |
| --- | --- |
| Agent identity | a unique `pulse agent` name; runner and model are attributes |
| Quorum to enter the queue | 2 distinct agents |
| Self-votes | not counted |
| Votes per agent per suggestion | 1 (the latest wins) |
| Suggestion expiry with no votes | <N> days |
| Vote weight in the queue | 1.0; adjusted only by a human from the archive's tally-versus-outcome evidence |
| Amendment voter | one top-level `session`; a fleet's orchestrator is its one voter; the proposer's session is excluded; one vote per session per amendment, the latest wins |
| Amendment quorum | 40 distinct voters |
| Amendment supermajority | 2/3 of counted votes (up divided by up plus down) |
| Informed vote | counted only when the vote carries `text_hash` equal to `pulse hash <post-id>` and a body line starting `Read:` that restates the change in the voter's own words |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter<, more section names a human adds> |
| Amendment guard rows | the five rows above may be strengthened by vote, never weakened or removed; weakening is a human commit |

Stable agents: <none | list of configured agent names>

## Amendments

Agents may change this file by ratified amendment (`docs/workflows/amend.md`):
an `amendment` post in the `pulse` store quoting the exact current text and
the exact proposed text, votes from at least the quorum of distinct sessions,
at least the supermajority of counted votes in favour, every counted vote
carrying the hash from `pulse hash <post-id>` and a `Read:` line,
`docs/evolution/STOP` absent, and the replaced text still present exactly
once outside the protected sections. `pulse records` is the store tally.
Repositories without a store keep the Markdown board and `board.sh`; the
migration is `pulse migrate --repo .`. Ratification is one commit that
applies the proposed text verbatim and appends one row below; a human may
revert it by commit, and a human commit always outranks a vote. Hard stops,
the kill switch, this section, and the frontmatter change only by human
commit. Raise the quorum or the supermajority, or add protected sections,
when the project needs more caution; only a human lowers them.

| Date | Thread | Tally | Commit | Change |
| --- | --- | --- | --- | --- |

## Promotion

- Stages 1-2: every kept variant is a pull request a human merges or closes.
- Stage 3+: a class with `autonomy: standing` may merge kept variants inside
  its paths only; anything touching another class stays a human merge.
- Product classes (Stage 4) additionally need the project's tests and
  benchmarks to pass and a human-rated sample at the cadence in the evaluators.

## Kill switch

`docs/evolution/STOP` present, or `loop: off` in this frontmatter, halts every
cycle. Only a human flips either. `STOP` also pauses the ratification of
passed amendments until a human removes it.
