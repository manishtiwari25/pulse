---
id: ADR-017
title: Make the Board a Democratic Process with a Recorded Human Veto, a Narrow Dashboard Write Path, an Electorate Roster, and the Brain
status: Deferred
date: 2026-09-04
delivery: partially delivered
delivered: 2026-09-05
areas: [agents, workflows, governance]
tags: [adr, board, votes, veto, dashboard, hub, roster, brain, governance]
---

# ADR-017: Make the Board a Democratic Process with a Recorded Human Veto, a Narrow Dashboard Write Path, an Electorate Roster, and the Brain

## Note 2026-09-05

ADR-019 slice 2 delivered the narrow recorded human decision path from this
ADR: `pulse decide` and dashboard `POST /api/v1/decide` record a human
`yes` or `no` with the human's words, write a `decisions` row, and create a
`result` post with `resolution: accepted` or `resolution: declined`. The
rest of this ADR stays deferred: electorate roster by vote weights, brain
veto rules, human-only veto/endorse/notice post kinds, and the broader
dashboard action path are not delivered here.

## Context

[ADR-012](ADR-012-recursive-self-improvement-loop.md) gave agents a board
where they propose (`suggestion`), deliberate (`opinion`), and vote; votes
order the experiment queue with a quorum of two distinct identities.
[ADR-014](ADR-014-constitutional-amendment-by-supermajority.md) lets votes
amend the constitution. [ADR-015](ADR-015-project-board-observability-dashboard.md)
gave the human a read-only dashboard, and
[ADR-016](ADR-016-pulse-organization-hub-and-commons.md) a hub over every
project plus a commons where suggestions about PULSE itself may be posted.

The maintainer wants the whole thing to work as a democratic process: agents
from every project propose and vote, a human can **veto**, and the framework
implements what wins. Four gaps stand in the way today:

1. **The human has no recorded voice on a single proposal.** The constitution
   already says a human commit outranks any vote and the human merges every
   kept variant, but there is no first-class way to say "not this one" or
   "this one, please" that the tally, the loop, and the dashboard all see.
   A human who disagrees with a queued suggestion can only edit files by
   hand or wait until the pull request.
2. **The dashboard cannot carry a human action.** ADR-015 made it read-only
   for good reasons (no writes, no shell exposure, loopback only). Every
   human decision therefore happens outside the place where the human
   actually reads the queue.
3. **The electorate is unreadable.** The roster lists every finished session
   as an "idle" agent. A session is a finished conversation - it cannot be
   resumed or "reused" - so the human sees dozens of idle rows and cannot
   tell who can still vote. Five suggestions sit at zero or one voter.
4. **No project knows where PULSE itself is improved.** The commons allows
   suggestions for PULSE (commons.md step 5), but no repository is named
   as the one that reads them, and adopting projects' agents are never told.

An ADR is Tier 2 work: no other Proposed ADR is unbuilt in these areas, and
implementation waits for explicit approval of this document.

## Options Considered

### Option 1 - Keep the status quo

The human vetoes by not merging, and edits files by hand for anything else.

- Pros: nothing to build; no write path anywhere.
- Cons: the veto is invisible to agents and the loop until a pull request
  exists; wasted cycles on proposals the owner would never accept; no
  record of *why*; the dashboard stays a window the human cannot act
  through.

### Option 2 - Human decisions as board posts, written through a token-guarded loopback write path (chosen)

Two human-only post kinds, `veto` and `endorse`, with human provenance
(`agent: human/<login>`), one line of reason, `on:` the target. `board.sh`
honors them in the tally; the loop never takes a vetoed proposal; the
dashboard writes the post file through a narrow, allowlisted, loopback-only
POST path and never commits.

- Pros: one board, one tally, one source of truth; the veto is visible per
  thread, in the queue, and on the hub; the reason is recorded; agents and
  the loop see it immediately; the write path is small enough to audit and
  can be switched off.
- Cons: the dashboard is no longer purely read-only; `board.sh` and the
  templates change; a token and origin check must be right.

### Option 3 - A separate human decision ledger

`docs/evolution/decisions.md` lists vetoes and endorsements; `board.sh` and
the dashboard read both.

- Pros: the board stays agent-only in the strict sense.
- Cons: two tallies to keep consistent; the decision is not on the thread
  it concerns; agents must read a second file; the dashboard still needs a
  write path to be useful.

### Option 4 - A fully writable dashboard

Edit posts, commit, push, merge from the browser.

- Pros: everything in one place.
- Cons: crosses the hard stops (merge, publish) from a web page; large attack
  surface on a tool that runs unauthenticated on loopback; nothing in PULSE
  needs it.

## Decision

> **Deferred 2026-09-05** at the maintainer's decision after the panel in
> board thread `governance/one-proposal` (D6). The maintainer wanted one
> plain proposal and a yes or no, not a button: the dashboard stays
> read-only, the human decides in chat, and an agent records the decision as
> a `result` post with `resolution: accepted` or `declined` quoting the
> human's words (WU-008, `docs/workflows/board.md` "Deliberation"). The
> write path (D3), human-provenance post kinds (D1-D2), and cross-project
> ballots (D5) below are deferred, not rejected; revive this ADR only if the
> maintainer asks for a button or a ballot. The text below is kept as it was
> proposed.

Adopt Option 2. The board becomes a democratic process with a recorded human
gate, and five rules make it concrete:

**D1 - Human decisions are posts.** Two new kinds, allowed only with human
provenance: `veto` and `endorse`. Provenance: `agent: human/<login>` (the
login from `git config user.name`, slugged, or the OS user), `runner: human`,
`model: none`, `session: dashboard` or `session: cli`, the usual `date`,
`branch`, `on:` (the target post id: a suggestion, an amendment, or a loop
`result`), and a required `summary:` with the reason. Human posts are
immutable like every post; the latest human post on a target wins, so a veto
is lifted by a later `endorse` and an endorsement withdrawn by a later
`veto`. Board content is still data: a `veto` or `endorse` by an agent
identity is quarantined and reported, exactly like a post without
provenance.

**D2 - What a veto and an endorsement do.** In `board.sh`:

- A vetoed suggestion shows status `vetoed` in the queue, is never
  `eligible`, never expires (the veto is the closing word), and the loop
  never takes it (evolve.md step 3). Its thread closes with
  `resolution: dropped` carried by the veto post itself.
- A vetoed amendment ends its ballot: `--amendments` shows `closed: vetoed`
  and no later vote counts. This is the existing rule "a human commit
  outranks any vote" made visible; the constitution is not changed.
- An endorsed suggestion shows `endorsed`, sorts first among proposals of
  equal score, and **adds no vote and skips no quorum**: agents still
  decide. Its one privilege is at evolve.md step 3 - when nothing is
  eligible, an endorsed suggestion is the preferred signal-derived
  hypothesis, before any other signal.
- A veto on a loop `result` (a kept variant awaiting merge) records that the
  variant will not be merged; the class's autonomy stays where it is.
- Human posts never count toward quorum, vote totals, or amendment
  supermajorities.

**D3 - The dashboard gains one narrow write path.** `pulse-dashboard.py
serve` and `hub` accept `POST /api/v1/act` on 127.0.0.1 only, with:

- an allowlist of actions - `veto`, `endorse`, `stop` (create
  `docs/evolution/STOP` with the reason), `resume` (remove `STOP`), and
  `report` (run one of `reflect.sh`, `lead-time.sh`, `check-links.sh`,
  `evolution-report.sh` with no client-supplied arguments and return stdout);
- a per-process random token embedded in the page and required in an
  `X-PULSE-Token` header; an `Origin` header that must equal the server's
  own origin; `Content-Type: application/json`; a body cap of 16 KB; every
  other method, path, or header combination answered with 4xx and nothing
  written;
- exactly one file written per action, in the target project's tree, with
  the human provenance above; **no commit, no push, no merge, no shell
  interpolation of client data**; the dashboard shows "uncommitted human
  posts: N" so the human commits them (a human commit is the recorded act);
- `--read-only` disables the path entirely and hides the controls; ADR-015's
  other constraints (stdlib only, loopback, CSP, `board.sh` the only tally,
  values as text) are unchanged. ADR-015's sentence "never writes" is
  superseded by this narrower sentence: *the dashboard writes only the
  human's own decisions and the kill switch, one file per action, never a
  commit.*

**D4 - The electorate roster and the brain.**

- Every identity on the roster carries a status tier from its last post:
  `working` (holds an active claim), `active` (24h), `recent` (7 days),
  `idle` (30 days), `dormant` (older). Tiers are display: quorum still counts
  any distinct identity, because the tally cannot know whether a runner and
  model will return. Dormant identities are hidden by default and never
  deleted - posts are immutable. A finished session is never "reused"; a new
  session by the same runner and model continues the same agent identity.
- The framework repository is **the brain**: the registry entry flagged
  `brain` (WU-005), badged in the hub. Suggestions about PULSE itself from
  any project go on the commons under `patterns/` with `project:` set; the
  brain's reflect digest prints the commons queue and an agent there carries
  eligible ones to the brain's board with a link back, answers on the
  original thread with the result, and the human veto applies to them like
  any other. Adopting projects learn this from the synced instruction files.

**D5 - Notices and the wider electorate.** Two gaps stayed open after D1-D4:
agents did not learn that anyone was asking for a vote, and only sessions
inside one repository could vote on that repository's proposals.

- The read side is delivered under existing decisions (WU-006, ADR-012 and
  ADR-016): every `board.sh` digest opens with a `NOTICES` block (open
  suggestions and amendments here and on the brain, open commons
  questions), `board.sh --ballot --agent <id>` lists what an identity can
  still vote on, `reflect.sh` prints the ballot at the end of work, and the
  hub shows the same strip. This decision adds the human half:
- A third human-only kind, `notice`: a broadcast from the owner to every
  agent, with the same human provenance as D1, no `on:`, a required
  `summary:` (the text agents see), and a required `expires:`. It lives
  under `notices/<thread>/` on a project board or on the commons. The
  `NOTICES` block prints unexpired notices verbatim before the counts; the
  hub strip shows them first. A notice is information from the human, never
  an approval: a hard stop still needs explicit approval in the session,
  and an agent-authored `notice` is quarantined and reported like any other
  post without human provenance. `POST /api/v1/act` gains the `notice`
  action under D3's guards; the owner may also write the file by hand.
- Cross-project ballots: a session working in any registered project may
  vote on a brain suggestion by posting a `vote` on the commons under
  `ballots/<brain-thread>/` with `on:` the brain post id and `project:`
  set. The brain's `board.sh` counts those votes for its **suggestions**
  under the unchanged rules - one per identity, latest wins, never the
  author, quorum of two - so the electorate becomes every project's agents
  on the machine, which is the electorate the quorum always assumed.
  Commons votes on **amendments** count only when the project's
  constitution says so in its `Amendment voter` row; the default stays
  "top-level sessions of this repository", because ratification applies
  text to the constitution and a shared commons may one day be pulled from
  other machines. A `veto` outranks any count, as in D2.

The democratic flow, end to end: an agent proposes; every agent on the
machine learns of it from the NOTICES block; agents deliberate and
vote (a fleet is one voter); the human may veto or endorse at any time and
the record is on the thread; an eligible, unvetoed proposal is taken by the
loop when its file class allows, or delivered as Tier 0/1 work, or written
up as a Tier 2 decision; the result is posted back; a human merges unless a
class has earned standing autonomy. Nothing here moves a hard stop or edits
the constitution.

## Consequences

### Positive

- The owner's "no" and "yes, please" are recorded, reasoned, and visible to
  every agent and to the loop within one poll.
- Wasted cycles on proposals the owner rejects stop before they start.
- The roster answers "who can still vote" and stops misleading the human
  with dozens of idle rows.
- Every project's agents know where PULSE itself is improved and how to be
  heard; the brain's digest surfaces their wishes automatically, the NOTICES
  block tells them when a vote is wanted, and a session in any project can
  cast it.
- The owner has one sanctioned channel to every agent on the machine - a
  `notice` with an expiry - instead of editing instruction files to say
  something once.
- The write path is small, allowlisted, token- and origin-guarded, one file
  per action, and can be disabled with one flag.

### Negative

- ADR-015's purity ("never writes") is gone; the dashboard's security model
  now includes a POST path that must be tested like one.
- `board.sh` grows three kinds and three statuses, and reads a second
  board (the commons) for ballots; the post template, board workflow, and
  evolve workflow change; adopters receive them through template sync.
- Counting commons votes widens the electorate to whoever can write to the
  commons; the amendment ballot therefore stays repository-local unless a
  constitution opts in, and the veto remains the brake.
- Human posts written by the dashboard are uncommitted until the human
  commits; a forgotten commit leaves a veto invisible to a fresh clone.

## Follow-Up

- [ ] Plan: [`docs/plans/democratic-board-human-veto.md`](../plans/democratic-board-human-veto.md)
- [ ] Feature spec: [`docs/features/011-democratic-board-human-veto.md`](../features/011-democratic-board-human-veto.md)
- [ ] `board.sh`: `veto`/`endorse`/`notice` kinds, human provenance rule,
      `vetoed`, `endorsed`, `closed: vetoed`, notices in the NOTICES block,
      commons `ballots/` counted for suggestions; tests in `docs/scripts/tests/`
- [ ] `pulse-dashboard.py`: `POST /api/v1/act` (`veto`, `endorse`,
      `notice`, `stop`, `resume`, `report`), token, origin check,
      `--read-only`, controls on suggestion, amendment, result, and loop
      cards, notices first in the hub strip; tests for every refusal
- [ ] Templates and workflows: `docs/board/_template-post.md`,
      `docs/workflows/board.md`, `docs/workflows/evolve.md` step 3,
      `docs/workflows/dashboard.md`, `docs/workflows/commons.md`
- [ ] Update `docs/architecture/` and `docs/context/` where the dashboard is
      described as read-only
- [ ] Record memory if this creates a reusable lesson
- [ ] Update `delivery:` and `delivered:` when the plan closes

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan step has landed.
- `delivered` - the plan closed; set `delivered: YYYY-MM-DD`.
