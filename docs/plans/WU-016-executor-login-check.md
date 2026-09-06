---
id: WU-016
title: Executor Login Check - a Stored Login Is Not a Working One
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-020
areas: [agents, workflows]
tags: [work-unit, runner, executors, login, recovery, keep-alive]
---

# Work Unit: Executor Login Check - a Stored Login Is Not a Working One

## Tier Declaration

- **Tier:** 1 - a check and a recovery loop on the remote-executor surface
  ADR-020 already draws; no new store, schema, credential path, or money.
- **Controlling ADR:** [ADR-020: Remote Executors](../decisions/ADR-020-remote-executors.md)
  (with [WU-014](WU-014-executor-faults-are-the-machines.md): a machine's
  fault is the machine's, never the job's).
- **Why this stays inside the ADR:** the hub already borrows a machine's
  hands over ssh, probes it, drains it on a fault, and re-probes an
  unreachable one with a doubling wait. This work unit adds one more probe
  (a real copilot call through the same verbs a job uses) and lets the
  existing backoff columns drive recovery from a login fault. The hands
  script is unchanged, so no machine needs re-pairing.
- **Hard stops touched:** none. Creating or placing the token stays the
  human's (`pulse executor login … --with-token`); the hub only validates
  what the human pipes and never prints or stores it. No machine is
  provisioned or deleted.

## Outcome

`pulse-vm1` sat `drained` for a day on 2026-09-06 with `logged_in=yes` in
every probe: the fine-grained token GitHub accepted as a login was refused by
Copilot at every worker launch (it lacks the "Copilot Requests" permission).
Nothing re-checked the machine, and only a human probe could have returned it
- to the same failure. When this work unit closes:

- placing a token tells the human at once whether Copilot will accept it,
  what account it belongs to, and when it expires;
- a machine drained for its login checks itself again with a growing wait and
  returns to service by itself the moment the token works;
- `pulse executor list` shows the last login verdict and the known expiry.

## Behavior

### User Story

As the human who runs the agents' machines, I want a login that GitHub
accepts but Copilot refuses to be caught when I place it, and a machine
drained for its login to come back by itself once I fix the token, so that
the fleet needs me only for the credential itself.

### Requirements

- [x] `github_token_facts(token)`: `GET /user` with the token before it is
      sent anywhere; 401/403 → refused, never sent (`executor.login` event
      with `step: github`); 200 → account name and the
      `github-authentication-token-expiration` header recorded in the
      `executor.login` event (`expires`, `user`, only when known); network
      failure → unknown, the token is sent anyway. Expiry within seven days
      prints `warning: the token expires in N day(s)`.
- [x] `login_check(con, name)`: `prepare` a one-commit bundle, `run` the
      copilot CLI (`runner_cmd`, `-p "Reply with exactly the word ok and
      nothing else." --no-ask-user --disable-builtin-mcps -s --model
      <worker_model>`, two minutes), always `clean`; verdict `ok`,
      `refused` (the copilot line matching `LOGIN_FAULT_RE`), `failed`,
      `hands`, or `unreachable`; one `executor.login-check` event.
- [x] `executor login` runs the check after the probe; a refused login
      fails the command (exit 1) and drains the machine.
- [x] A refused login (at login time, at a worker launch, or at a recheck)
      sets `backoff_minutes` 15 → doubling → 360 and `next_probe_at`;
      `reprobe_due` reruns the login check on a `drained` machine whose
      `probe_note` names a login fault and whose wait is over, and a passing
      check sets it `active` (`executor.recovered`).
- [x] A passing shallow probe keeps a login-drained machine drained
      (`fault: login refused, awaiting the login check; ok: …`).
- [x] `pulse executor probe <name> --login` runs the deep check by hand; the
      plain `probe` of a login-drained machine includes it. `PULSE_AGENT`
      names the actor when set.
- [x] `pulse executor list` prints `login ok|FAILED at <time>: <note>`, the
      known expiry, and a `WARNING` when it is near.

### Interface Changes

- CLI: `executor probe <name> --login`; richer `executor login` output;
  `executor list` login line. All inside ADR-020's executor surface.
- Events: `executor.login-check`, `executor.recovered`; `executor.login`
  gains optional `expires`, `user`, and `step: github`.

### Data Changes

None: no schema change (the dashboard requires an exact schema match, so
login facts live in events and recovery reuses `backoff_minutes` /
`next_probe_at`).

### Edge Cases

- GitHub unreachable from the hub: the token is still sent; the fact is
  marked unknown, never treated as refused.
- The check's copilot run times out or crashes for a non-login reason:
  `kind: failed`, the machine is not drained; a drained machine waits longer.
- The machine is unreachable during a recheck: `kind: unreachable`, the wait
  doubles; the unreachable path's own probe is unchanged.
- A machine drained by a human (`drained by the human`) or for another fault
  (old hands) is never rechecked: `login_drained()` needs a login fault in
  `probe_note` and a scheduled `next_probe_at`.
- The check spends one Copilot request per run on the worker model; at the
  six-hour cap that is four requests a day per broken machine.

## Steps

- [x] `login_check`, `tiny_bundle`, `fault_line`, `github_token_facts`,
      `expiry_warning`, `login_drained`, `last_login_check`, `login_expiry`
      in `docs/scripts/pulse.py`.
- [x] `executor_login`: facts before sending; check after storing.
- [x] `note_executor_run` fault path seeds the backoff; `reprobe_due`
      rechecks; `probe_executor` keeps a login-drained machine drained.
- [x] CLI `--login`; `executors_text` login line.
- [x] Tests: `LoginCheckTestCase`, `GithubTokenFactsTestCase` in
      `docs/scripts/tests/test_pulse_login.py` (fake copilot through the
      real hands script; GitHub patched).
- [x] Docs: `docs/workflows/executors.md`, `docs/context/pulse-store.md`,
      `docs/memory/mistakes.md`.

## Implementation Prompt

- **Repository and boundary:** the PULSE runner (`docs/scripts/pulse.py`)
  and its tests; the remote-executor surface of ADR-020. The hands script
  `docs/scripts/hosted/pulse-hands.sh` must not change (a change forces
  re-pairing every machine).
- **Files likely to change:** `docs/scripts/pulse.py`,
  `docs/scripts/tests/test_pulse_login.py`, `docs/workflows/executors.md`,
  `docs/context/pulse-store.md`, `docs/memory/mistakes.md`.
- **Patterns to reuse:** `ssh_run` and the `prepare`/`run`/`clean` verbs;
  `LOGIN_FAULT_RE` / `login_fault`; the `unreachable` backoff in
  `reprobe_due`; `HandsFixture` with a fake worker on `runner_cmd`.
- **Constraints:** no schema bump; the token never appears in output, logs,
  or events; the check uses the worker model and the CLI's own flags only.
- **Do not:** provision or delete machines, create tokens, or re-activate a
  machine on a shallow probe alone.

## Rollback Plan

- **Baseline:** commit `4dab4ce` plus the ADR-024 D8-D10 commit; the store
  at schema 3; `pulse-vm1` `drained` with a login fault.
- **Trigger:** the login test module or the full suite red; a machine
  re-activated while Copilot still refuses it; the token appearing in any
  output.
- **Reversal:** `git revert` the WU-016 commit (one commit, `pulse.py`,
  tests, docs). No store change to undo: the extra events are harmless
  history, and `backoff_minutes` / `next_probe_at` on a drained machine are
  ignored by the previous code.
- **State safety:** no dependencies, migrations, or deployments; the hands
  are untouched; the only external calls are `GET /user` on GitHub and the
  copilot CLI on the executor.
- **Recovery verification:** `python3 -m unittest discover -s
  docs/scripts/tests -p 'test_pulse*.py'` green; `pulse executor list`
  prints as before.

## Acceptance Criteria

- [x] The behavior above is delivered and verified: the login module passes
      (28 tests), the full suite passes, and the real check against
      `pulse-vm1` names the fault in seven seconds
      (`copilot refuses the login on pulse-vm1: Error: Authentication
      failed …`) while the machine stays drained with a 15-minute recheck.
