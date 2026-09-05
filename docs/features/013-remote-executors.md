---
id: FEAT-013
title: Remote Executors - A Second Machine Lends Its Hands to the One Store
status: Accepted
tier: 2
date: 2026-09-05
delivered:
priority: High
areas: [agents, workflows, governance]
tags: [feature, runner, executors, ssh, scaling, agent-first]
---

# Feature: Remote Executors - A Second Machine Lends Its Hands to the One Store

## Summary

The PULSE runner learns to run a job on another machine the maintainer owns.
The hub - the machine with `~/.pulse/pulse.sqlite` - keeps everything that
thinks: the board, the votes, the leases, the roster, the human's decisions,
and the landing of branches. An **executor** is a pair of hands: the hub
ships a project to it as a git bundle over ssh, runs the same denied-tool
worker there under the same caps, verifies there, fetches the commits back
into the real checkout as `pulse/<agent>/<job>`, and removes the clone. The
local machine is the first executor of the same kind. This is
[ADR-020](../decisions/ADR-020-remote-executors.md), the second machine
ADR-018 and ADR-019 left for their slice 3.

## User Story

As the maintainer, I want to add one more VM and have the agents work from it
as well, so that the machine keeps improving my projects with more hands and
more credits while I stay the only one who merges, provisions, and decides.

## Requirements

### Functional

- [ ] `pulse executor add NAME --ssh USER@HOST[:PORT] --workdir DIR
      [--workers N] [--projects a,b|*]` registers an executor; `list`,
      `probe`, `drain`, `activate`, `remove` manage it; every change is an
      event; `local` exists implicitly and cannot be removed.
- [ ] A job assigned to an ssh executor runs the phases prepare (bundle,
      scp, clone with no remote), worker (`copilot -p` with the deny list,
      `--no-ask-user`, `--disable-builtin-mcps`, `--max-ai-credits`, the
      wall-time cap), collect (`PULSE_RESULT.md`, commit count), verify (the
      project's `verify_cmd`, capped), land (the hub fetches over `ssh://`
      after `renew_lease`), clean (`rm -rf` on the VM, bundle removed on the
      hub).
- [ ] The `runs` row and the `result` post name the executor.
- [ ] The brief carries the project's ballot; `VOTE: <post-id> up|down
      <reason>` lines in `PULSE_RESULT.md` are cast by the hub as the
      worker's votes, marked `cast_by: hub-from-result`, only for ids on the
      ballot the hub rendered into that brief and only with a reason;
      self-votes and off-ballot ids are skipped with a log line.
- [ ] The daemon runs up to `sum(workers)` jobs at once across active
      executors, one leased job per project, one store connection per run;
      a job goes only to an executor whose `projects` allows it; an eligible
      healthy remote is chosen first and `local` is the fallback when none
      is.
- [ ] An ssh failure before the worker starts is an `error` run: job
      requeued, nothing landed, no cap spent; a loss of contact after the
      worker started is a `failed` run that spends the cap; three
      consecutive errors mark the executor `unreachable`, and the daemon
      re-probes it with a doubling wait until a probe succeeds or a human
      runs `activate`.
- [ ] `pulse executor probe` enforces the hardening checklist (single-purpose
      user; the hub's key alone with `no-agent-forwarding`,
      `no-port-forwarding`, `no-X11-forwarding`, and a forced command rooted
      in `<workdir>`; no clone with a remote in `<workdir>`; `git` and the
      copilot CLI answer); `activate` refuses an executor whose last probe
      failed.
- [ ] A real VM is enrolled only after the gate: the daemon restarted with
      WU-009, five consecutive non-`error` local runs with at least two
      useful results, and the cap - not credits - the limit. The gate is
      evidence for the human's decision, not a lock on slice 1.
- [ ] `jobs_per_day` counts worker runs on every executor; `max_ai_credits`
      and `minutes_per_job` apply per run everywhere.
- [ ] `pulse status` and the read-only dashboard show executors, their
      status, slots in use, and runs today.

### Non-Functional

- [ ] **Security:** a VM never pushes, never has a remote, never holds a
      repository credential or a token for the store; it holds its own
      Copilot login and the hub's ssh public key, both placed by the human;
      the key carries `no-agent-forwarding`, `no-port-forwarding`,
      `no-X11-forwarding`, and a forced command rooted in `<workdir>`; the
      hub connects with `BatchMode=yes` and `ClearAllForwardings=yes` so
      nothing prompts and nothing tunnels; `--projects` limits which bundles
      may travel to it; egress is narrowed to what the copilot CLI and the
      project's verify need. The hardening is part of the boundary, not a
      runbook detail.
- [ ] **Reliability:** every phase is capped; a VM that dies mid-run leaves
      an `error` or `timeout` run, a requeued job, and a clone the hub
      removes at the next contact; the hub never blocks on a VM.
- [ ] **Performance:** a single-branch, `--no-tags` bundle; transfer time is
      measured in slice 2 and recorded in the plan.
- [ ] **Testability:** an `ssh`/`scp` shim in the tests exercises the whole
      ssh path locally; no test needs a network.

## Scope

### Area

`docs/scripts/pulse.py` (schema, commands, `Executor` classes, `run_job`,
`daemon`, `write_brief`, `parse_result_file`), its tests,
`docs/scripts/pulse-dashboard.py` (one read-only panel and endpoint),
`docs/context/pulse-store.md`, `docs/workflows/pulse.md`, a new
`docs/workflows/executors.md`.

### Out of Scope

- Replicating the store or running a second daemon (ADR-020 Option 2).
- A lease or posting API over HTTP (Option 3).
- A persistent mirror per executor, a second hub, or executor-side
  scheduling (ADR-020 slice 3, not decided).
- Any change to the deny list, the caps' values, or what stays human.

## Interface Changes

- CLI: `pulse executor add|list|probe|drain|activate|remove`;
  `pulse work --executor NAME` and `pulse work --dry-run --executor NAME`.
- Brief: a **Ballot** section and the `VOTE:` line format in rule 7.
- Result file: `VOTE:` lines recognized.
- `pulse status`: an **executors** block.
- Dashboard: `/api/v1/executors` and an **Executors** panel, read-only.

## Data Changes

- New table `executors`; new column `runs.executor` (default `local`);
  schema version 2 with an idempotent migration; new event actions
  `executor.add|probe|drain|activate|remove|unreachable`. Posts and events
  stay immutable; no existing row changes meaning.

## Decisions

- [ADR-020](../decisions/ADR-020-remote-executors.md) - scale hands, not
  brains.
- [ADR-019](../decisions/ADR-019-agent-first-pulse-store-and-runner.md) -
  one store per machine; the runner; what stays human.
- [ADR-018](../decisions/ADR-018-nudge-harness.md) - no server; git as the
  transport; the entry conditions for a second machine.
- [WU-009](../plans/WU-009-runner-loop-leaks.md) - renewable,
  generation-checked leases; `error` and `stale` runs.

## Edge Cases

- The hub cannot reach the VM at prepare: `error`, requeue, no cap spent;
  after three, `unreachable`, re-probed with a doubling wait; `local` keeps
  taking jobs meanwhile.
- The link drops during the worker: the ssh process exits non-zero; the run
  is `failed` because credits may have burned, so it spends the cap and the
  job requeues only within `attempts`; the worker may keep running on the
  VM, but only the hub fetches, so nothing it does can land; its clone is
  removed at the next contact.
- The worker exceeds the wall-time cap: the hub kills the ssh session and
  records `timeout`; the VM-side process is terminated by the wrapper's
  process group, which the runbook verifies.
- The lease expires during a long verify on a slow VM: `renew_lease` before
  landing fails, the run is `stale`, nothing lands (WU-009).
- A `VOTE:` line names the worker's own post, an id that was not on the
  rendered ballot, or has no reason: skipped with a log line; the rest are
  cast with `cast_by: hub-from-result`.
- The probe finds the key without the forwarding restrictions or the forced
  command, or a clone with a remote in `<workdir>`: the executor stays
  `drained` or turns `unreachable`, `activate` refuses, the probe note says
  what to fix.
- `--projects` excludes the job's project: the executor is not eligible;
  the job waits for one that is, `local` included.
- The VM has no copilot login: the worker is rejected at launch, the run is
  `error`, the probe reports the missing login, the executor turns
  `unreachable` after three; the runbook names the fix.
- Two executors finish the same project's two jobs at once: impossible by
  the one-leased-job-per-project rule; the second job waits.

## Rollback Strategy

- **Baseline:** the WU-009 commit; schema 1; the live store; no VM enrolled.
- **Trigger:** a failed test suite; a branch landed by anything but the hub's
  fetch; a clone with a remote or a credential on a VM; a stale result
  landing; the maintainer's no.
- **Reversal:** `git revert` the slice's commits; stop and restart the
  daemon; the added table and nullable column are ignored by the reverted
  code; for a VM, `drain`, `remove`, remove the hub's key, log out of
  copilot there (runbook).
- **State safety:** no new dependency; immutable posts and events; nothing
  the hub needs lives on a VM.
- **Recovery verification:** tests pass at the baseline count; `pulse
  status` shows `local` only; `pulse work --dry-run` lands nothing.

## Generated Prompts

- None yet; the plan's steps are the prompt. One is generated only if a
  handoff to another agent needs it.

## Acceptance Criteria

- [ ] All functional requirements above pass in the test suite through the
      ssh shim.
- [ ] The maintainer's first VM runs one job end to end (slice 2), and the
      bundle size, transfer time, and credit report are recorded in the plan.
- [ ] Every hard stop is unchanged: no push, no remote, no credential, no
      merge by the runner.

## Verification Plan

```bash
python3 -m unittest discover -s docs/scripts/tests
python3 docs/scripts/pulse.py executor list
python3 docs/scripts/pulse.py executor probe vm1
python3 docs/scripts/pulse.py work --dry-run --executor vm1
python3 docs/scripts/pulse.py status
```
