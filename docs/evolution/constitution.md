---
id: CONSTITUTION
title: PULSE Evolution Constitution
status: Active
date: 2026-09-03
controlling_adr: ADR-012
loop: on
limits: none
---

# PULSE Evolution Constitution

Rules the self-improvement loop can never change in the PULSE source
repository, which runs the loop as its own first adopter. Every line here is a
human commit or a ratified amendment (see Amendments below). A variant that
touches this file, the evaluators, or any immutable path below fails the
constraint check and is discarded. The paths are also listed in
`.github/CODEOWNERS` so GitHub requires the maintainer's review on them.

The values below are the maintainer's starting defaults for Stage 1. They
change by a human commit or, outside the protected sections, by a ratified
amendment under `docs/workflows/amend.md` (ADR-014).

## Hard stops

Identical to the hard stops in `AGENTS.md`. The loop never crosses them and
never proposes a change to them.

- destructive or irreversible actions, production, deploy, publish, merge
- credentials, hosted-data deletion, broad auth, billing, or data migration
- infrastructure, major dependency upgrades, policy-sensitive security
- purchases, ambiguous evidence

## Immutable paths

| Path                                                                                                                                                                                     | Why                                                                                                 |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `docs/evolution/constitution.md`                                                                                                                                                         | this file: a human commit or a ratified amendment (`docs/workflows/amend.md`), never a loop variant |
| `docs/evolution/evaluators.md`, `docs/evolution/fixtures/`                                                                                                                               | the definition of better                                                                            |
| `docs/evolution/STOP`                                                                                                                                                                    | kill switch; only a human creates or removes it                                                     |
| `docs/scripts/` except `pulse-dashboard.py` and `tests/test_pulse_dashboard.py`                                                                                                          | evaluation and reporting tools; editing them could game the checks. The dashboard is a product class, not a check |
| `docs/decisions/`, `docs/versions.md`                                                                                                                                                    | decisions and versions are human records                                                            |
| `AGENTS.md` sections "Critical Rules", "Proportional Lifecycle Tiers", "Agents' Board and Self-Improvement Loop", "Sandbox Recommendation", "Rollback Planning"; `CLAUDE.md`; `.github/` | rules and hosting humans own                                                                        |
| `LICENSE`                                                                                                                                                                                | legal, if one is ever added                                                                         |

## Editable scope by stage

PULSE is the template, so its loop may edit the framework prose that adopters
receive through sync - skills, workflows, shared prompts, guides, and memory -
inside the limits above. PULSE ships one product: the read-only dashboard and
hub (ADR-015, ADR-016). It is the product class below, at Stage 4 from its
first cycle, measured by the `## Product evaluators` section of
`evaluators.md`; only its autonomy is earned.

| File class            | Paths                                                                     | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --------------------- | ------------------------------------------------------------------------- | ----- | -------- | --------------------- | ------------------- | --------- |
| memory                | `docs/memory/`                                                            | 1     | human    | 5                     | 0                   | 0         |
| skill prose           | `docs/skills/*/SKILL.md`, `docs/skills/README.md`                         | 1     | human    | 8                     | 0                   | 0         |
| workflows             | `docs/workflows/` except `rollback.md` and `sandboxed-agent-execution.md` | 1     | human    | 8                     | 0                   | 0         |
| shared prompts        | `docs/prompts/shared/`                                                    | 1     | human    | 8                     | 0                   | 0         |
| guides and context    | `docs/guides/`, `docs/context/`, `docs/architecture/`                     | 1     | human    | 5                     | 0                   | 0         |
| operating guide prose | `AGENTS.md` outside the immutable sections, `README.md`, `docs/README.md` | 1     | human    | 10                    | 0                   | 0         |
| dashboard (product)   | `docs/scripts/pulse-dashboard.py`, `docs/scripts/tests/test_pulse_dashboard.py` | 4     | human    | 8                     | 0                   | 0         |

## Budget

`limits: none` (frontmatter): no budget limit of any kind binds in this
repository. Tokens, wall time, cycles per day, and plateau never cap, pause,
or end a cycle or the run; only the stop rules do (`docs/evolution/STOP`, a
constitution violation, a hard stop, an instruction-shaped board post, a
human interrupt). Maintainer decision, 2026-09-03; ADR-012 Amendment 2. The
rows below are kept so a cap is one edit away (`limits: budget` plus a
number). `Runs per fixture` and `Tolerance` are measurement settings and
still apply.

| Limit                          | Value                                                                                                                  |
| ------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| Tokens per cycle               | unlimited                                                                                                              |
| Wall time per cycle            | unlimited                                                                                                              |
| Cycles per day                 | unlimited - continuous mode: when a cycle ends, the next begins                                                        |
| Plateau                        | no count: after any run of discards, post a `finding` and change the hypothesis or file class; never a stop or a pause |
| Runs per fixture               | 3; the median is used                                                                                                  |
| Tolerance on non-target scores | 5% worse allowed                                                                                                       |

## Sandbox

Every cycle runs on an experiment branch inside the strongest sandbox the
runner supports and reports the actual boundary. An unsandboxed cycle is
recorded as such in the archive and is never promoted with standing autonomy.

## Vote rules

| Rule                              | Value                                                                                                                                                                                |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Agent identity                    | `<runner>/<model>`                                                                                                                                                                   |
| Quorum to enter the queue         | 2 distinct agent identities                                                                                                                                                          |
| Self-votes                        | not counted                                                                                                                                                                          |
| Votes per identity per suggestion | 1 (the latest wins)                                                                                                                                                                  |
| Suggestion expiry with no votes   | 14 days                                                                                                                                                                              |
| Vote weight in the queue          | 1.0; adjusted only by a human from the archive's tally-versus-outcome evidence                                                                                                       |
| Amendment voter                   | one top-level `session`; a fleet's orchestrator is its one voter; the proposer's session is excluded; one vote per session per amendment, the latest wins                            |
| Amendment quorum                  | 40 distinct voters                                                                                                                                                                   |
| Amendment supermajority           | 2/3 of counted votes (up divided by up plus down)                                                                                                                                    |
| Informed vote                     | counted only when the vote carries `text_hash` equal to `docs/scripts/board.sh --hash <proposal>` and a body line starting `Read:` that restates the change in the voter's own words |
| Not amendable by vote             | Hard stops, Kill switch, Amendments, frontmatter                                                                                                                                     |
| Amendment guard rows              | the five rows above may be strengthened by vote, never weakened or removed; weakening is a human commit                                                                              |

Stable identities: none.

## Amendments

Agents may change this file by ratified amendment (ADR-014,
`docs/workflows/amend.md`): an `amendment` post on the board quoting the
exact current text and the exact proposed text, votes from at least the
quorum of distinct sessions, at least the supermajority of counted votes in
favour, every counted vote carrying the hash of the proposed text and a
`Read:` line, `docs/evolution/STOP` absent, and the replaced text still
present exactly once outside the protected sections. `docs/scripts/board.sh
--amendments` is the only tally. Ratification is one commit that applies the
proposed text verbatim and appends one row below; a human may revert it by
commit, and a human commit always outranks a vote. Hard stops, the kill
switch, this section, and the frontmatter change only by human commit.

| Date | Thread | Tally | Commit | Change |
| ---- | ------ | ----- | ------ | ------ |

## Promotion

- Stages 1-2: every kept variant is a pull request the maintainer merges or
  closes.
- Stage 3+: a class with `autonomy: standing` may merge kept variants inside
  its paths only; anything touching another class stays a human merge.

## Kill switch

`docs/evolution/STOP` present, or `loop: off` in this frontmatter, halts every
cycle. Only a human flips either. The loop is `off` until the first baseline
is recorded in `archive.md`. `STOP` also pauses the ratification of passed
amendments until a human removes it.
