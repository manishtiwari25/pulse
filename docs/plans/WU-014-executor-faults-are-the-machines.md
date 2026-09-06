---
id: WU-014
title: Executor Faults Are the Machine's, Not the Job's
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-020
areas: [agents, workflows]
tags: [work-unit, runner, executors, hands, keep-alive]
---

# Work Unit: Executor Faults Are the Machine's, Not the Job's

On 2026-09-06, in the fifteen minutes between the daemon restarting on the
ADR-022 runner and the hands on `pulse-vm1` being refreshed, two improve jobs
died. Each run ended `error` for a reason that had nothing to do with the
job - first `pulse-hands: verb not allowed: key` (old hands), then `worker
rejected at launch: … re-authenticate` (the VM's Copilot login had expired,
though the probe said `logged_in=yes` because a config file existed). Every
such error was charged to the **job**: three attempts, `failed`, its
suggestion marked *tried*. The **machine** stayed `active`, took the next
job, and did it again. A keep-alive runner with a broken pair of hands would
grind the whole queue to `failed` overnight.

## Tier Declaration

- **Tier:** 1 - runner behavior inside the executor surface ADR-020 decided:
  its `active`/`drained`/`unreachable` states, its rule that `local` is the
  fallback, and its rule that a runner error never spends the cap.
- **Controlling ADR:** [ADR-020: Remote Executors](../decisions/ADR-020-remote-executors.md)
  (Accepted 2026-09-05).
- **Why this stays inside the ADR:** no new table, column, state, verb, or
  credential. A fault the machine caused moves it to the `drained` state
  ADR-020 already defines ("takes no job until a human probe passes"), the
  job returns to the queue as ADR-020's runner-error rule already says, and
  `local` picks it up as ADR-020's fallback rule already says. The probe
  gains one check on a field the hands already print.

## Outcome

- When a remote pair of hands refuses a verb the hub needs, or its worker
  is rejected at launch for a login reason, the **executor** is drained
  with the reason on its row, the **job** goes back to the queue with its
  attempt returned, and the next daemon loop runs it on `local` (or on
  other hands). One human command - `pulse pair … --name <x>` or
  `pulse executor login <x> …` - brings the machine back, as it already
  does today.
- `pulse executor probe` fails hands that predate ADR-022 (no `keys=`
  field) with the refresh command, so a hub upgrade names the machines it
  left behind before any job reaches them.

## Behavior

1. `SshExecutor` raises `ExecutorFault` (a `PulseError`) when the hands
   answer `verb not allowed` to any verb, or when the `key` verb fails: the
   machine's program is wrong, retrying cannot help.
2. `run_job` treats a launch rejection (exit within `LAUNCH_SECONDS`, no
   commit, no result) whose output names a login problem
   (`re-authenticate`, `not logged in`, `authentication`, `unauthorized`,
   `401`, `Copilot Requests`) **on a remote executor** as an executor fault
   too. On `local` nothing changes: there is no machine row to drain and the
   daemon log already shows the reason.
3. An executor fault ends the run `error` (nothing was spent), with a
   summary that names the machine and the human command that repairs it;
   `note_executor_run` sets the executor `drained`, writes
   `fault: <reason>` to `probe_note`, counts one error, and logs
   `executor.drained`; the job is requeued **without** counting the attempt
   (`attempts` decremented; a fault never contributes to the three that fail
   a job) and the `run.finish` event carries `fault: true`.
4. A `drained` executor takes no lease (already true), so the daemon's next
   loop leases the job to `local` or to other active hands (already true).
   A human `probe`, `pulse pair`, or `pulse executor login` that passes
   re-activates it (already true).
5. `probe_executor` adds one check: hands whose probe output has no `keys`
   field predate ADR-022 and fail with "hands predate ADR-022 (no keys=
   field); refresh them with `pulse pair <admin@host> --name <name>`".
6. Transport errors (`ExecutorUnreachable`) keep their own path: three in a
   row mark `unreachable` with backoff, as before.

## Steps

- [x] `pulse.py`: `ExecutorFault`; `SshExecutor.call` classifies
      `verb not allowed`; `prepare` raises it for the `key` verb; launch
      rejection classifier `LOGIN_FAULT_RE`; `run_job` fault path;
      `note_executor_run(fault=...)`; probe `keys` check.
- [x] Tests: old hands (shim `PULSE_SHIM_OLD_HANDS=1`) drain the executor
      and give the job back, then `local` runs it; a login-shaped launch
      rejection on hands does the same; on `local` the job is still charged;
      the probe fails old hands and passes new ones; three transport errors
      still mark `unreachable`, not `drained`.
- [x] Docs: `docs/workflows/executors.md` (faults, the probe row);
      `docs/context/pulse-store.md` (`probe_note` `fault:` prefix).
- [x] Close: ADR-020 follow-up line; this unit's `delivered:`.

## Implementation Prompt

Build exactly the Behavior list above in `docs/scripts/pulse.py` and
`docs/scripts/tests/test_pulse.py`. Standard library only. Do not add a
status, a column, a verb, or a setting. Keep `ExecutorUnreachable`
untouched. Run `python3 -m unittest discover -s docs/scripts/tests` before
committing.

## Rollback Plan

- **Baseline:** `main` at `71f4ea1`; files: `docs/scripts/pulse.py`,
  `docs/scripts/tests/test_pulse.py`, `docs/workflows/executors.md`,
  `docs/context/pulse-store.md`, this unit, ADR-020's follow-up line.
- **Trigger:** any test red; a real run whose worker failed for a job
  reason being classified as a machine fault (a healthy machine drained).
- **Reversal:** `git revert` of this unit's commit; a drained executor is
  re-activated by `pulse executor probe <name>` (human).
- **State safety:** no schema change; store rows are only updated in ways
  the CLI already performs (`status`, `probe_note`, `attempts`); no key,
  credential, or remote file is touched.
- **Recovery verification:** `python3 -m unittest discover -s
  docs/scripts/tests` green; `pulse executor probe pulse-vm1` ok.

## Acceptance Criteria

- [x] Old hands or an expired login on remote hands: executor `drained`
      with `fault:` in `probe_note`, job `queued` with its attempt returned,
      next `work` runs it on `local`.
- [x] A launch rejection on `local` still charges the job.
- [x] The probe fails hands without `keys=` and names `pulse pair`.
- [x] Transport errors unchanged (`unreachable` after three).
- [x] Full suite green.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests`
- `python3 docs/scripts/pulse.py executor probe pulse-vm1`
- `git diff --check`; `bash docs/scripts/check-links.sh`

## Closure

Delivered 2026-09-06. `test_pulse.py` 67 tests green; the real
`pulse-vm1` passes the new probe check (`keys=-` present). The two jobs
that died before this unit (`job-20260906T075314Z-212690`,
`job-20260906T080518Z-8e9fd1`) stay `failed` - the store does not rewrite
history - and their suggestions count as tried; a `finding` on the board
records why.
