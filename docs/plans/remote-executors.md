---
id: PLAN-020
title: Remote Executors - The Hub Keeps the Store, a VM Lends Its Hands
status: Approved
tier: 2
date: 2026-09-05
delivered:
tags: [plan, runner, executors, ssh, scaling, agent-first]
---

# Plan: Remote Executors - The Hub Keeps the Store, a VM Lends Its Hands

## Tier

Tier 2 - a new store table, a new machine class, infrastructure the maintainer
provisions, and a Copilot login he places: infrastructure and credentials are
hard stops, and no ADR covers a second machine. Decision:
[ADR-020](../decisions/ADR-020-remote-executors.md). The maintainer said yes on
2026-09-05 (`pulse decide 20260905T103746Z-fable-scale-09051203 yes`, result
`20260905T112407Z-human`); slice 1 may start, slice 2 waits for his VM.

## Goal

A second machine the maintainer owns runs PULSE workers for every project he
allows, through the one store on the hub, with no second brain, no server, no
repository credential on the VM, and every hard stop unchanged. The local
machine becomes the first executor of the same kind, so one code path is
tested and used everywhere.

## Context

- Code: `docs/scripts/pulse.py` (`run_job`, `work_once`, `daemon`,
  `pick_worker`, `lease_job`, `renew_lease`, `finish_job`, `write_brief`,
  `parse_result_file`), `docs/scripts/tests/test_pulse.py`,
  `docs/scripts/pulse-dashboard.py` (read-only views of the store).
- Docs: `docs/context/pulse-store.md` (schema), `docs/workflows/pulse.md`
  (runner procedure), a new `docs/workflows/executors.md` (human runbook).
- Constraints: the store stays one file on the hub; posts and events stay
  immutable; the worker deny list, `--no-ask-user`, `--disable-builtin-mcps`,
  `--max-ai-credits`, and the wall-time cap apply on every executor; a VM
  never pushes, never has a remote, never holds a repository credential; the
  hub fetches. `jobs_per_day` counts worker runs on every executor.
- Assumptions: the maintainer can reach the VM over ssh with a key
  (tailnet, VPN, or public address); the VM has `git` and the copilot CLI
  with a login; the project bundle fits the link (single branch, `--no-tags`).
- Known unknowns, to measure in slice 2: bundle size and transfer time for
  `amistio`; ssh flakiness; how Copilot credit accounting appears per login.

## Steps

### Slice 1 - the executor abstraction, local first

- [x] **Schema.** Add `executors` (`name`, `kind`, `address`, `workdir`,
      `workers`, `projects`, `status`, `last_probe_at`, `probe_note`,
      `runs_done`, `runs_failed`, `created_at`, `created_by`) and
      `runs.executor` (default `local`) in `pulse.py`'s schema with an
      idempotent migration (`schema` version 2); `pulse schema` prints it.
- [x] **Commands.** `pulse executor add NAME --ssh USER@HOST[:PORT] --workdir DIR
      [--workers N] [--projects a,b|*]`, `pulse executor list`,
      `pulse executor probe NAME`, `pulse executor drain NAME`,
      `pulse executor activate NAME`, `pulse executor remove NAME` (refused
      while a run is on it). Every change is an event. `local` is implicit
      and cannot be removed.
- [x] **Interface.** One `Executor` class with `prepare(job, project) ->
      clone_ref`, `run_worker(cmd, env, timeout)`, `result_file()`,
      `commit_count(base)`, `verify(cmd, timeout)`, `fetch_source()` (a path
      or an `ssh://` URL the hub fetches from), `cleanup()`. `LocalExecutor`
      wraps today's code unchanged; `SshExecutor` implements each phase with
      `git bundle create` + `scp` + `ssh git clone`, `ssh` with
      `BatchMode=yes`, `ConnectTimeout=15`, the same environment names, and
      `ssh rm -rf` on cleanup. Any ssh failure raises `PulseError`, which
      `run_job` already records as an `error` run (requeue, no cap spend).
- [x] **`run_job` through the interface.** Same phases, same statuses, same
      `renew_lease` before verify and before landing; the hub lands with
      `git fetch --no-tags <fetch_source> HEAD:refs/heads/pulse/<agent>/<job>`.
      The `runs` row and the `result` post carry the executor.
- [x] **Votes from a distance.** `write_brief` renders the project's ballot
      into the brief (id, thread, summary, current votes) and documents
      `VOTE: <post-id> up|down <reason>`; `parse_result_file` collects
      `VOTE:` lines; `run_job` casts them with `create_post(kind="vote")` as
      the worker, marked `cast_by: hub-from-result`, accepting only ids that
      were on the rendered ballot, with a reason, never the worker's own
      post; anything else is skipped with a log line. The direct
      `pulse vote` path from WU-009 stays for local workers.
- [x] **Scheduling.** `daemon` keeps one thread (and one store connection)
      per free worker slot across active executors (`sum(workers)`); a job
      is assigned to the executor with a free slot whose `projects` allows
      the job's project, an eligible healthy remote first and `local` as
      the fallback when none is; one leased job per project holds. An ssh
      failure before the worker starts is an `error` (requeue, no cap
      spent); a loss of contact after the worker started is `failed`
      (cap spent, requeue within `attempts`). Three consecutive `error`
      runs on one executor set `status = 'unreachable'`; the daemon skips
      it and re-probes with a doubling wait (1, 2, 4 minutes, up to
      `interval`) until a probe succeeds or a human activates it.
- [x] **Hardening and the probe.** `pulse executor probe` checks the
      single-purpose user, the `authorized_keys` options
      (`no-agent-forwarding`, `no-port-forwarding`, `no-X11-forwarding`,
      a forced command rooted in `<workdir>`), that `<workdir>` holds no
      clone with a remote, and that `git` and the copilot CLI answer; the
      hub connects with `BatchMode=yes`, `ConnectTimeout=15`, and
      `ClearAllForwardings=yes`. `activate` refuses an executor whose last
      probe failed.
- [x] **Surfaces.** `pulse status` lists executors with status, slots in
      use, runs today; the dashboard gets a read-only **Executors** table
      (`/api/v1/executors`, on the hub's Agents page).
- [x] **Tests.** An `ssh` shim on `PATH` in the tests (`ExecutorTestCase`,
      seven tests through the real `pulse-hands.sh`) that runs the
      remote command locally in a temp "remote" directory, so
      `SshExecutor` is exercised end to end without a network: bundle,
      clone, worker, verify, fetch, cleanup; ssh failure before the worker
      -> `error` run, requeue, no cap; loss after the worker started ->
      `failed`, cap spent; three errors -> `unreachable`, skipped, backoff
      re-probe; a late result after a re-lease -> `stale`, nothing lands,
      no double finish (the existing local test covers the shared
      `run_job` path); `VOTE:` lines become votes only for ballot ids,
      self-votes and off-ballot ids skipped; two executors running two
      projects concurrently keep one leased job per project and land two
      branches; `--projects` refuses a project not on the list; the probe
      refuses a key without the forwarding restrictions.
- [x] **Docs.** `docs/context/pulse-store.md` (table, column, settings),
      `docs/workflows/pulse.md` (the executor phases),
      `docs/workflows/executors.md` (the human runbook with the hardening
      checklist: provision a VM with `git >= 2.30`, `python3`, the copilot
      CLI; a single-purpose user; `copilot` login on that user; the hub's
      public key alone with the restrictions above; narrow egress to what
      the CLI and verify need; `pulse executor add`, `probe`, `drain`,
      `remove`; what to watch in the daemon log; recovery when a VM dies
      mid-run - a passing probe sweeps idle job directories), decisions
      index, ADR-018 and ADR-019 follow-ups pointing here. Also written for
      slice 2: `docs/scripts/hosted/azure-provision.sh` and
      `cloud-init.yaml`, the human-run provisioning pair.

### Gate between the slices (evidence for the human, not a lock on slice 1)

- [x] The daemon has been restarted with WU-009 and, on `local`, at least
      five consecutive runs are not `error`, at least two of them `kept` or
      otherwise useful, and the daily cap - not credits - is what stops
      more work. Evidence (store, 2026-09-05, `executor = local`): the last
      twelve runs hold no `error`; `run-20260905T164644Z-b87e63` (3
      commits, kept), `run-20260905T165134Z-5f105b` (1 commit, kept),
      `run-20260905T170541Z-1cecbc`, `run-20260905T170821Z-406b33`,
      `run-20260905T171203Z-3fe763` (1 commit, kept),
      `run-20260905T171946Z-bb8270`, `run-20260905T174343Z-228b5e` all
      `done`; the others are `failed` (verify or owner paths) or `timeout`,
      the worker's outcomes, not the runner's. The daily cap is off by the
      maintainer's choice (`jobs_per_day 0`), so time and credits per run
      bound the work; no run ended for credits since `worker_model` was set.

### Slice 2 - the first VM

- [x] The VM exists (2026-09-06). The owner had recorded in project-light's
      MEM-008 that agents on this machine may create compute in the sandbox
      subscription (service principal from 1Password, one fleet group named
      by the subscription's standard in `germanywestcentral`, purpose tags
      only, never a person's name), and asked in chat that the executor
      follow it (decision `20260906T041719Z-human` on proposal
      `20260905T202621Z-fable-scale-09051203`). An agent ran `bash
      docs/scripts/hosted/azure-provision.sh --group <fleet group>
      --location germanywestcentral --name pulse-vm1 --pubkey
      ~/.ssh/pulse_hands_ed25519.pub --tag operator=pulse-agents --tag
      purpose=pulse-executor --tag created-by=copilot-cli --tag
      ttl=persistent --yes`: `Standard_B2ms`, 64 GiB, ssh from the hub's
      address only, cloud-init clean (`errors: []`), Node 22, copilot CLI
      1.0.83, user `pulse` locked to `pulse-hands` with `restrict`, sshd
      `Match User pulse` hardening in force, git never signs or prompts.
      The first attempt lacked `- default` in cloud-init's `users:` and had no
      admin user; it was removed with the script's new `--delete` mode (the
      group stayed) and recreated from the fixed file. The hub reaches it by
      an ssh alias (`Host pulse-vm1`, dedicated passphrase-free key), and its
      host key came through `az vm run-command` (`--host-key`), not a blind
      first connection. Probe from outside: `hands=ok git=2.43.0
      copilot=1.0.83 keys_ok=yes remotes=0 disk_free_mb=59622`; an unlisted
      verb, a plain command, a git fetch outside a job clone, and a tty
      request are all refused.
- [x] Enrolled: `pulse executor add pulse-vm1 --ssh pulse@pulse-vm1
      --workdir /srv/pulse --workers 1 --projects '*'`; `probe` reports
      `copilot is not logged in` and keeps it `drained`; `work --dry-run
      --executor pulse-vm1` answers `idle: executor pulse-vm1 is drained`.
      The hands now report `logged_in=no` until `copilot login` ran (was
      `unknown`, which the hub let pass and would have spent a job's three
      attempts on a machine that cannot run the worker).
- [ ] The maintainer logs the CLI in on the single-purpose user - `ssh -t
      pulseadmin@pulse-vm1 sudo -iu pulse copilot login` - and runs `python3
      docs/scripts/pulse.py executor probe pulse-vm1`, which activates it.
- [ ] One job runs on it; the plan records bundle size, transfer time, and
      the run's credit report.
- [ ] Close: `delivered:` here, `delivery: delivered` in ADR-020, ADR-018
      and ADR-019 slice 3 marked delivered by ADR-020.

## Rollback Plan

- **Baseline:** the commit that carries WU-009 (`docs/scripts/pulse.py`
  schema 1, `runs` without `executor`); the live store
  `~/.pulse/pulse.sqlite`; the daemon process on the hub; no VM enrolled.
- **Trigger:** any failure in `python3 -m unittest discover -s
  docs/scripts/tests`; a run on an ssh executor that lands a branch the hub
  did not fetch itself, or that leaves a clone with a remote; a VM holding
  a repository credential; a `stale` result landing; or the maintainer's no.
- **Reversal:** slice 1 is one commit series on a branch until the tests
  pass; revert it with `git revert` (never a reset). The schema migration
  adds a table and a nullable column only, so the reverted code reads the
  store unchanged; `pulse executor` rows stay as history. Stop the daemon,
  revert, restart the daemon. For slice 2: `pulse executor drain vm1`, wait
  for its runs, `pulse executor remove vm1`, remove the hub's key from the
  VM, and log out of copilot there - the human's steps, in the runbook.
- **State safety:** no dependency is added (stdlib, `git`, `ssh`, `scp`);
  posts and events remain immutable; the VM never holds a remote or a
  credential, so removing it removes nothing the hub needs; bundles are
  deleted after each run.
- **Recovery verification:** the test suite passes at the baseline count;
  `pulse status` shows no executor but `local`; `pulse work --dry-run`
  prepares a local clone and lands nothing.

## Acceptance Criteria

- [x] A job runs to completion on an `SshExecutor` through the shim in
      tests: bundle in, worker run, verify, commits fetched by the hub as
      `pulse/<agent>/<job>`, clone removed, `result` posted with the
      executor named (`test_a_remote_run_lands_from_the_hands_...`).
- [x] A VM never pushes and never has a remote: the hands' probe counts
      clones with remotes and fails on any; the deny list is on the worker
      command on every executor; the hub fetches, the hands never push.
- [x] An ssh failure before the worker starts is an `error` run: job
      requeued, cap unspent; a loss of contact after the worker started is
      `failed` and spends the cap; three errors mark the executor
      `unreachable` with a doubling re-probe wait; a probe or a human brings
      it back; `local` stays the fallback (`test_losing_the_hands_...`).
- [x] `VOTE:` lines become that agent's votes only for ids on the rendered
      ballot, marked `cast_by: hub-from-result`; self-votes and off-ballot
      ids are skipped.
- [x] The probe enforces the hardening checklist (single-purpose user,
      restricted key options, forced command, no remotes in `<workdir>`) and
      `activate` refuses an executor whose last probe failed.
- [x] Two executors run two projects at once; one leased job per project
      holds; both land (`test_the_daemon_fills_every_free_slot_...`).
- [x] `--projects` is honored; `jobs_per_day` counts runs on every executor.
- [x] Docs updated; the runbook with the hardening checklist exists in slice
      1; the gate's five clean local runs are recorded.
- [ ] The first VM has run one job (slice 2; the VM exists and is enrolled, awaiting the maintainer's `copilot login`).
- [ ] On closure, `delivered:` is set here and ADR-020's `delivery:` mark is
      updated.

## Verification

```bash
python3 docs/scripts/tests/test_pulse.py                           # runner + executors through the shim (95 tests, 2026-09-05)
python3 -m unittest docs.scripts.tests.test_pulse_dashboard        # dashboard incl. /api/v1/executors (44 tests)
bash -n docs/scripts/hosted/pulse-hands.sh docs/scripts/hosted/azure-provision.sh
python3 docs/scripts/pulse.py executor list                        # local + any enrolled
python3 docs/scripts/pulse.py executor probe vm1                   # slice 2, on the hub
python3 docs/scripts/pulse.py work --dry-run --executor vm1        # bundle + clone on the VM, no worker
python3 docs/scripts/pulse.py status                               # executors, slots, runs today
```
