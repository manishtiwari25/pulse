---
id: ADR-020
title: Scale Hands, Not Brains - Remote Executors Over SSH for the PULSE Runner
status: Accepted
accepted: 2026-09-05
date: 2026-09-05
delivery: partially delivered
delivered:
areas: [agents, workflows, governance]
tags: [adr, runner, executors, ssh, scaling, second-machine, leases, agent-first]
---

# ADR-020: Scale Hands, Not Brains - Remote Executors Over SSH for the PULSE Runner

## Context

The maintainer's vision, in his words (2026-09-04): "my goal is the agents
should be independent of project, they will be aware of all the projects and
they can work on the project whenever the need is there, like an open source
project ... then it will make scaling easy: I can add one more VM on remote
and then we can use the agent process from there so everyone works
collectively." And on 2026-09-05: "how can we make it more agent run? and my
idea of scaling the agent infra?"

Two decisions already point at this and stop short of it:

- [ADR-018](ADR-018-nudge-harness.md) slice 3, "more machines", needs "its
  own yes and its own runbook", says **no server** ("cross-machine state is
  git ... a coordinator service is revisited only if git contention is ever
  measured"), keeps credentials and merging human, and sets entry
  conditions: atomic renewable leases, ownership generations, stable task
  ids, duplicate suppression, stale-result rejection, and "stop publishing
  when disconnected", proven by tests.
- [ADR-019](ADR-019-agent-first-pulse-store-and-runner.md) then moved the
  truth from Git-tracked Markdown into **one SQLite store per machine** with
  one daemon, and left slice 3 as "replication of the store or git remotes,
  with leases proven under contention first".

ADR-019 changed the shape of the problem. The board, the votes, the quorum,
the leases, the agents' identities, and the human's decisions are rows in one
file. A second machine with its own store would be a second brain: two
tallies that disagree, two leasers that race, two rosters, two places for the
human to look. The question is no longer "how do two machines share git"
but "how does a second machine join **without becoming a second brain**".

What exists today, after [WU-009](../plans/WU-009-runner-loop-leaks.md):
`lease_job` takes a job atomically with an expiring lease and a generation;
`renew_lease` extends a lease the caller still holds; `finish_job` refuses a
stale generation; a run whose lease was retaken is `stale` and lands nothing;
jobs dedupe on `source`; runner errors requeue without spending the cap.
Every ADR-018 entry condition but "stop publishing when disconnected" is now
code with tests - and that one is only meaningful once a network sits
between the runner and the worker.

Facts that bound the design. The runner already isolates a worker in a
throwaway clone with no remote, runs `copilot -p` with a deny list
(`git push`, `git remote`, `gh`, built-in MCP servers), caps its credits and
minutes, verifies with the project's own check, and **fetches** the commits
into the real checkout as `pulse/<agent>/<job>` - the checkout pulls, the
clone never pushes. Git can `bundle` a branch into one file and `fetch` over
`ssh://` from any path, both without a hosted remote or a token. A Copilot
login is per machine; a second machine with its own login is the only way to
add AI credits, which - not CPU - is the scarce resource. Day one of the
runner (2026-09-05) shows the daily cap, not compute, as today's ceiling:
2 jobs a day across 3 projects, 4 jobs waiting.

## Options Considered

### Option 1 - Do nothing new: raise the caps on the one machine

`pulse settings set jobs_per_day 6` and `workers 2` triple today's
throughput with no code.

- Pros: zero risk, zero code, the human's command; it is the right first
  move whatever else is decided.
- Cons: one login, one credit budget, one machine that must be awake; the
  vision of adding a VM stays a vision. Done anyway, not instead.

### Option 2 - Replicate the store: every machine a full PULSE

Litestream, LiteFS, rqlite, or rsync snapshots of `pulse.sqlite`; each
machine runs its own daemon against its copy.

- Pros: each machine works alone; the hub can sleep.
- Cons: a new dependency (a hard stop on its own); two writers on `jobs`,
  `agents`, and the tally - exactly the split brain above; LiteFS and rqlite
  elect a primary anyway, which is Option 4 with more moving parts; rsync
  snapshots lose writes. Rejected until a single-writer design is measured
  to be insufficient.

### Option 3 - A lease API: remote runners talk to the hub over HTTP

The dashboard already serves HTTP and has one narrow write
(`POST /api/v1/decide`). Add `/api/v1/jobs/lease|renew|finish` and
`/api/v1/posts`; a VM runs `pulse work --hub URL --token T`, clones from a
git remote with its own read credential, and pushes `pulse/*` branches with
a deploy key.

- Pros: N machines, each autonomous; a familiar shape.
- Cons: the coordinator service ADR-018 said to revisit only after measured
  contention; a token on every VM and a repository credential on every VM
  (read plus push to `pulse/*`) - ADR-019 keeps credentials off the worker
  and pushing off the runner; the store becomes a network service to
  authenticate, harden, and version; the most code of the four.

### Option 4 - Remote executors over SSH: scale hands, not brains (chosen)

The hub - the machine that holds the store - keeps everything that thinks:
the store, the daemon, planning, leases, votes and the tally, pruning, the
human's decisions, and landing. A second machine is an **executor**: a pair
of hands that runs one worker in one clone and hands the commits back. The
hub reaches it over `ssh`, ships the clone as a git bundle, runs the same
denied-tool worker there under the same caps, runs the project's verify
there, **fetches** the result back over `ssh://`, and removes the clone.

- Pros: one brain, so no replication, no second tally, no second leaser -
  leases now protect concurrent runs inside one daemon, which the WU-009
  code already does; no server and no network-exposed store; the VM holds
  **no repository remote and no repository credential** - it receives a
  bundle and is fetched from - only its own Copilot login, placed by the
  human, and the hub's ssh public key; the VM cannot push, has no remote,
  no `gh`, and is capped per job exactly as today; git stays the transport
  as ADR-018 wanted; `local` becomes the first executor, so one code path
  serves both; adding a machine is one human command plus one key.
- Cons: the hub is a single point of failure - when it sleeps nothing runs
  anywhere (accepted: ADR-019 already calls it "the agents' machine"); the
  hub must reach the VM (a VPN, a tailnet, or a public address the human
  provides); a bundle travels per job (a single-branch bundle is small; a
  persistent mirror on the VM is a later optimization); the daemon becomes
  concurrent (one connection per run, WAL; one leased job per project still
  holds - raised to a per-project limit, `parallel_per_project`, by WU-019); a bundle of a private repository leaves the hub - to a machine the
  maintainer owns, the same trust as his laptop, and he decides per executor
  which projects may travel.

## Decision

Adopt Option 4, and do Option 1 today by the human's hand.

Accepted by the maintainer on 2026-09-05 with `pulse decide
20260905T103746Z-fable-scale-09051203 yes` (result `20260905T112407Z-human`),
on the panel's agreed proposal: D1-D3 as delivered in WU-009; D4 as amended -
hardening is the boundary, `unreachable` with doubling backoff, `local` is
the fallback, a real VM only after five clean local runs. Implementation
follows [PLAN-020](../plans/remote-executors.md), slice 1 first.

### 1. Executors

`executors` is a new table in the store: `name` (unique), `kind`
(`local` or `ssh`), `address` (`user@host[:port]` for ssh), `workdir` (a
directory the executor owns), `workers` (concurrent runs it may take),
`projects` (`*` or a list of slugs whose bundles may travel there), `status`
(`active`, `drained`, `unreachable`), `last_probe_at`, `probe_note`,
`runs_done`, `runs_failed`, `created_at`, `created_by`. The `local`
executor exists implicitly with `workers = settings.workers`; a human adds
the rest:

```bash
pulse executor add vm1 --ssh work@vm1.tailnet --workdir /srv/pulse --workers 2 --projects amistio,project-light
pulse executor probe vm1        # ssh, git --version, copilot --version, disk, login present
pulse executor drain vm1        # finish running jobs, take no new ones
pulse executor list
```

Adding, draining, and removing an executor is a human act, like
`pulse init`; the daemon never enrolls a machine.

### 2. A run on an executor

`run_job` gains one variable: where the hands are. The phases stay the same
on every executor - prepare the clone, write the brief, run the worker,
collect the result and the commits, verify, land, clean up - and each phase
has a local and an ssh form:

- **prepare:** `git bundle create <job>.bundle <default-branch>` on the hub;
  `scp` it to `<workdir>/<job>/`; `ssh` `git clone -q <job>.bundle clone`
  (a clone from a bundle has a file remote, which the runner removes, so the
  clone has no remote at all); the brief is written on the hub and copied.
- **worker:** `ssh <address> 'cd <workdir>/<job>/clone && copilot -p ...'`
  with the same deny list, `--no-ask-user`, `--disable-builtin-mcps`,
  `--max-ai-credits <max_ai_credits>`, the same environment names, and
  the hub's wall-time cap on the ssh process; `ssh -o BatchMode=yes -o
  ConnectTimeout=15`, never a password prompt.
- **collect:** `scp` `PULSE_RESULT.md` back; `ssh git rev-list --count`.
- **verify:** the project's `verify_cmd` over `ssh` in the clone, capped.
- **land:** the hub runs `git fetch --no-tags ssh://<address>/<workdir>/<job>/clone
  HEAD:refs/heads/pulse/<agent>/<job>` into the real checkout - the hub
  pulls, the VM never pushes - after `renew_lease` succeeds, exactly as
  WU-009 does locally.
- **clean:** `ssh rm -rf <workdir>/<job>`; the bundle on the hub is removed.

The `runs` row records `executor`. A `result` post says where it ran.

### 3. Votes from a distance

A worker on a remote executor cannot reach the store. The brief therefore
carries the ballot (rendered by the hub when the brief is written), and the
worker answers with `VOTE: <post-id> up|down <one-line reason>` lines in
`PULSE_RESULT.md`; the hub casts them as that agent's votes when it collects
the result, the way it already posts the worker's `FINDING:` and
`QUESTION:` lines. The hub accepts a `VOTE:` line only for a post that was
on the ballot it rendered into that brief, with a reason, never on the
worker's own post; anything else is skipped with a log line. Each cast vote
carries `cast_by: hub-from-result`, so a vote that travelled in a file is
distinguishable from one cast directly. The same lines work on the local
executor; the direct `pulse vote` path from WU-009 stays. A vote still
approves nothing.

### 4. Disconnection

"Stop publishing when disconnected" (ADR-018) becomes concrete. An ssh
failure **before the worker starts** (bundle, copy, clone, brief) is a runner
**error** - the job requeues, nothing lands, no cap is spent (WU-009). A
loss of contact **after the worker started** is a **failed** run: credits
may have burned, so it spends the cap, and the job requeues only within
`attempts`. Either way nothing lands: `renew_lease` before landing and the
generation check in `finish_job` (WU-009) make a late result `stale`. Three
consecutive errors on one executor mark it `unreachable`; the daemon skips
it and probes it again with a doubling wait (one minute, two, four, up to
`interval`) until a probe succeeds or a human sets it `active`. A worker
left running on a VM that lost its hub finishes alone and its clone is
removed at the next contact; nothing it did can reach any checkout, because
only the hub fetches.

### 5. Caps and concurrency

`jobs_per_day` stays one machine-wide human cap and now counts worker runs
on every executor. Each executor has its own `workers`; the daemon runs up to
the sum concurrently, `parallel_per_project` leased jobs per project (1 by
default, as today; [WU-019](../plans/WU-019-runner-throughput.md)), each run
on its own store connection, and a freed slot is refilled while slower runs
are still going. A job goes to an eligible remote executor first and to
`local` when no healthy remote is eligible; `local` is the fallback, never
skipped. `max_ai_credits` and `minutes_per_job` apply per run wherever it
runs. The credits come from each executor's own Copilot login - accounts the
maintainer holds - inside caps he sets: tool use inside a grant, as ADR-019
§5 already says.

### 6. What stays human

Provisioning a machine and paying for it; placing its Copilot login and the
hub's ssh key; adding, draining, and removing an executor; naming which
projects may travel to it; the caps; merging any `pulse/*` branch;
`STOP`; and every hard stop in `AGENTS.md`. The runner never enrolls a
machine, never copies a credential, never pushes, and never lets a VM reach
a checkout or a hosted remote.

The executor's hardening is part of this boundary, not a runbook detail
(panel, 2026-09-05): a single-purpose OS user on the VM; the hub's key alone
in its `authorized_keys`, with `no-agent-forwarding`, `no-port-forwarding`,
`no-X11-forwarding`, and a forced command that only runs the executor's
phases rooted in `<workdir>`; the hub connects with `BatchMode=yes` and
`ClearAllForwardings=yes`; the VM holds no repository credential, no store,
and no token for either; the `projects` allow-list decides which bundles may
leave the hub at all; network egress narrowed to what the copilot CLI and
the project's verify need. `pulse executor probe` checks what it can see of
this (user, forwarding options, forced command, no remotes in `<workdir>`)
and refuses to activate an executor that fails.

**Amendment 1 (ADR-021, 2026-09-06).** ADR-021 keeps this section's owner
boundary and adds one machine-checkable exception: an agent may run
`pulse pair` only when `PULSE_AGENT` is set and the store holds a human
`pulse decide ... yes`, no more than seven days old, on a proposal whose body
contains the exact line `pair: <executor>`. The command checks that row before
touching anything and records the decision's post id in `executor.pair`. The
exception does not cover credentials: `pulse executor login` and any Copilot
token remain the human's hard stop.

### 7. Delivery in slices

- **Slice 1 - the abstraction and the local executor:** `executors` table
  and commands; `LocalExecutor` and `SshExecutor` behind one interface;
  `run_job` through it; `VOTE:` lines cast by the hub from the rendered
  ballot only; the ballot in the brief; concurrency per executor;
  `unreachable` with backoff; `pulse status` and the read-only dashboard
  show executors; `docs/workflows/executors.md` written, with the hardening
  checklist above and the probe that enforces it. Tests drive `SshExecutor`
  through a shim that runs `ssh`/`scp` locally, so no test needs a network.
  No real VM in this slice.
- **Gate between the slices (panel, 2026-09-05):** the maintainer has
  restarted the daemon with WU-009, the local runner shows at least five
  consecutive runs that are not `error` with at least two `kept` or
  otherwise useful results, and the daily cap - not credits - is what
  stops more work. The gate is evidence for the human's decision to enrol
  a machine; it is not a lock on slice 1.
- **Slice 2 - the first VM:** the maintainer provisions it, logs the
  copilot CLI in, places the hub's key with the restrictions above, runs
  `pulse executor add` and `probe`; the first job runs there and the plan
  records bundle size, transfer time, and the credit report.
- **Slice 3 - not decided here:** a second hub, or a persistent mirror per
  executor, only if the single hub is measured to hurt.

## Consequences

### Positive

- One brain: the tally, the quorum, the roster, the proposals, and the
  human's decisions stay in one place; adding machines never adds a second
  truth.
- Credits scale with logins the maintainer holds; throughput scales with
  `workers` per executor; every hard stop is unchanged.
- Nothing on a VM can reach a checkout or a remote; the worst a compromised
  VM can do is fail its own runs.
- ADR-018 slice 3 and ADR-019 slice 3 close through one mechanism.

### Negative

- The hub is a single point of failure and must be awake and reachable.
- The daemon becomes concurrent; the tests must prove the leases under it.
- A bundle of a private repository leaves the hub for a machine the
  maintainer owns; he restricts it per executor with `--projects`.
- ssh reachability (tailnet, VPN, firewall) is the human's problem to solve
  before `pulse executor add`.

## Follow-Up

- [ ] Plan: [`docs/plans/remote-executors.md`](../plans/remote-executors.md)
      (rollback plan inside); spec:
      [`docs/features/013-remote-executors.md`](../features/013-remote-executors.md).
- [ ] On acceptance: `status: Accepted`; ADR-018 and ADR-019 follow-ups
      point their slice 3 here.
- [x] Update `docs/context/pulse-store.md` (the `executors` table, `runs.executor`)
      and `docs/workflows/pulse.md` when slice 1 lands; write
      `docs/workflows/executors.md` with the hardening checklist in slice 1
      (2026-09-05; `delivery: partially delivered`).
- [ ] Deliberation that shaped §3-§7: question
      `20260905T100816Z-fable-scale-09051203-2`, opinions
      `20260905T103137Z-panel-reliability-0905a`,
      `20260905T103228Z-panel-security-0905a`,
      `20260905T103311Z-panel-value-0905a` (`pulse show <id>`).
- [ ] Record memory if the first VM teaches a reusable lesson (bundle size,
      ssh flakiness, credit accounting).
- [x] [WU-014](../plans/WU-014-executor-faults-are-the-machines.md)
      (2026-09-06): a fault the machine caused - hands that refuse a verb,
      a worker that cannot log in - drains the executor and returns the
      job's attempt, inside the `drained` state and the `local` fallback
      this decision defines; the probe fails hands without `keys=`.
- [ ] Update `delivery:` and `delivered:` when the plan closes.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - slice 1 landed.
- `delivered` - slice 2 closed with the first real executor probed.
- `retired` - superseded or abandoned.

Built by: [`docs/plans/remote-executors.md`](../plans/remote-executors.md).
