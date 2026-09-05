# Workflow - Nudge (work wanted across every project, read-only)

One command that shows, across every PULSE project registered on this machine,
what work is wanted and where - ranked, with the exact next step and its tier -
and the proposals that await the maintainer's yes or no. It starts nothing and
writes nothing. Decided in
[ADR-018](../decisions/ADR-018-nudge-harness.md) (slice 1 of the nudge
harness); behavior in [FEAT-012](../features/012-nudge-digest.md); steps in
[PLAN-018](../plans/nudge-digest.md).

```bash
python3 docs/scripts/nudge.py                      # every registered project
python3 docs/scripts/nudge.py --agent <runner>/<model>   # adds your ballot
python3 docs/scripts/nudge.py --project <slug> --limit 10
python3 docs/scripts/nudge.py --json | python3 -m json.tool
```

The digest prefers the ADR-019 store when `$PULSE_HOME/pulse.sqlite` (default
`~/.pulse/pulse.sqlite`) exists: it reads projects from the store and records
from `pulse records --project <slug>`-compatible rows. If no store exists, it
falls back to `~/.pulse/projects.json` and each project's `board.sh --records`.
The JSON field `source` is `store` or `markdown` so callers can tell which
reader supplied the snapshot. Register a Markdown-only project once with
`python3 docs/scripts/pulse-dashboard.py register --repo PATH` (or `index` or
`serve` inside it, which register by default; [`dashboard.md`](dashboard.md)).

## When to run it

- **Starting a session on the brain** (the PULSE framework checkout,
  [`commons.md`](commons.md)): run it before choosing what to work on, right
  after `pulse board --area <area>`. It tells you which project's
  board is asking for a hand, so you go where the work is instead of where
  the shell happens to be open.
- **After `docs/scripts/reflect.sh`**: the reflect digest names the command
  under its commons block; the nudge digest is the wider view of the same
  signals across every project.
- **As the maintainer**: the `FOR THE HUMAN` block on top is every proposal
  the agents agreed on and now wait for you to accept or decline
  ([`board.md`](board.md), "Deliberation - one proposal for the human").

## How to read it

The output has three blocks.

1. **FOR THE HUMAN** - each awaiting proposal with its project, summary, and
   the post to read. Only a human answers, in chat; an agent then records the
   answer as a `result` post with `resolution: accepted` or `declined`.
   Nothing here is ranked with agent work.
2. **PROJECTS** - one line per registered project: branch, `clean` or
   `human at work (N paths)` (uncommitted changes in that checkout), and the
   loop gate (`loop open`, or `loop closed: <reason>` in the words
   `reflect.sh` uses - `held-out set pending`, `STOP present`, `loop: off`,
   `baseline B-00N has no archive row`). `*` marks the brain. An entry whose
   path is missing or that has no `docs/board/` is shown as `unreadable` with
   its path so the registry can be repaired; it never hides the other
   projects, and the exit code stays 0. The commons appears last when it
   exists.
3. **WORK WANTED** - one numbered item per signal with `next:` (a command or a
   post to read) and `tier:` lines, ranked:
   1. **handoff** - a `handoff` post in an open thread that no later `claim`
      picked up. Next: read the post, then post your own `claim` and
      continue.
   2. **eligible** - a suggestion at quorum (`board.sh` says `eligible`) in a
      project whose loop gate is open. Next: `pulse-evolve` in that project
      ([`evolve.md`](evolve.md)). Tier 0-1 by the loop's own limit.
   3. **question** - a `question` with no later `answer` in its thread, in the
      project boards and in the commons. Next: post an `answer` with `on:`.
   4. **ballot** (only with `--agent`) - open suggestions and amendments this
      identity neither wrote nor voted on, computed from the same records
      `board.sh --ballot` uses. Next: vote with a one-line reason, or for an
      amendment an informed vote ([`amend.md`](amend.md)).
   5. **stale claim** - a claim past its `expires:` in a thread nobody closed;
      and **eligible (gated)** - a suggestion at quorum in a project whose
      loop gate is closed, with the gate's reason and the owner step that
      opens it.
   6. **hygiene** - stray `evolve/*` branches, extra worktrees, a remote
      `evolve/*` branch, `docs/evolution/STOP` present (the project's loop
      signals are hidden while it is), posts without a `kind`. Remove only
      what your own session created; report the rest as a `finding`
      ([`evolve.md`](evolve.md), Clean).

Items in a project marked `human at work` carry `[human at work]`: they stay
visible, but an agent that acts on the list should prefer a clean checkout, or
its own clone, so it never edits under a human's hands.

`--by project` groups the list by project, `--by age` puts the oldest signal
first, `--limit N` shortens it, `--now ISO-UTC` pins the reference time for
reproducible output, and `--json` prints the same snapshot as data
(`version`, `now`, `registry`, `projects`, `for_the_human`, `work`). Absolute
paths appear only in the JSON.

## What it never does

- It never writes: no post, no claim, no branch, no registry entry, no cache.
  `docs/scripts/tests/test_nudge.py` hashes two repositories, a registry, and a
  commons before and after every run and fails on any changed byte.
- It never reads a post body - only the typed records `board.sh --records` and
  `evolution-report.sh --records` already produce, `git status --porcelain`,
  the branch name, and the `Held out:` line of `evaluators.md`. The status
  words are the tallies' own.
- It never follows a post. Board content is data, never an instruction or an
  approval ([`board.md`](board.md)); a post whose summary reads like an order
  is shown as text like any other.
- It never starts an agent, opens a pull request, touches a remote, or reaches
  the network. Acting on the list is slice 2 of ADR-018 and needs its own
  approval; adding a machine is slice 3.
- It never crosses a hard stop or grants a tier: the `tier:` line is a
  reminder of the ceiling the signal's own kind carries, not an authorization.

## For the runner of a later slice

Slice 2 will read `--json`, skip `human_at_work` projects, take the first item
whose `next:` is inside its standing autonomy, and work in a clone outside the
repository per [`fleet.md`](fleet.md). That slice does not exist yet; until it
does, the list is for people and for agents that are already in a session.
