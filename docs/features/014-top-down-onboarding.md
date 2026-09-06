---
id: FEAT-014
title: Top-Down PULSE - Onboard a Project With One Command, Pair a Machine With One Command
status: Proposed
tier: 2
date: 2026-09-06
delivered:
priority: High
areas: [agents, workflows, governance]
tags: [feature, runner, onboarding, pairing, executors, daemon, agent-first]
---

# Feature: Top-Down PULSE - Onboard a Project With One Command, Pair a Machine With One Command

## Summary

Two composite commands make the maintainer's top-down picture real without
moving any gate. `pulse onboard <path>` registers a project, lays down the
loop's files from the templates when they are missing, queues the onboarding
job and the first `reflect` job right away, and prints the two things only
a human may do (record the evaluators' baseline, set `loop: on`).
`pulse pair <admin>@<host> --name <executor>` turns a Linux machine that
already exists into a pair of hands exactly as `cloud-init.yaml` would have at
first boot, enrolls it, and probes it. The daemon stays the one supervisor;
the dashboard is untouched. Both commands are the human's unless the store
holds the human's `decide yes` on a proposal naming the target. This is
[ADR-021](../decisions/ADR-021-top-down-onboarding.md).

## User Story

As the maintainer, I want to point PULSE at a project and have the agents
start within minutes, and to add a machine with one command, so that the
fleet grows like one brain with more hands while I keep the merges, the
loop's gates, the credentials, and the decisions.

## Requirements

### `pulse onboard`

1. Runs `init` unchanged (register, onboarding job) and accepts its options.
2. When `docs/evolution/` is entirely absent in the checkout, creates it
   from the three templates with `loop: off`; when complete, touches nothing
   and says so; when partial, refuses (exit 2, missing names listed) before
   any write. Never commits or stages; prints the new paths.
3. Queues one `reflect` job (`source onboard-reflect:<slug>`, priority 19)
   once per project, ever - inserted directly, not through the planner.
4. Prints the human gates: baseline (`B-` row in `evaluators.md`), `loop:
   on`, and `pulse service install` when no daemon heartbeat is fresh.
5. A second run is a no-op that prints the same gates.

### Planner rule

6. `plan_jobs` plans a `reflect` for a project that has never had one, even
   when the daily reflect already ran elsewhere; the cadence rule keeps
   applying afterwards. The existing "no job waiting" gate still applies, so
   for a plain-`init` project the reflect follows its onboarding job on the
   next loop.

### `pulse pair`

7. Generates `~/.ssh/pulse_hands_ed25519` (no passphrase) when missing;
   never uses another key.
8. Over the admin session with `sudo`, idempotently: user `pulse` (locked
   password, `/bin/bash`), `/usr/local/bin/pulse-hands` from this checkout
   (mode 0755, root-owned), `<workdir>` owned by `pulse` (0750), exactly one
   `authorized_keys` line with `command=` and `restrict`, the sshd
   `Match User pulse` block as the drop-in
   `/etc/ssh/sshd_config.d/60-pulse-hands.conf`, `commit.gpgsign false` and
   `core.askPass ""` for `pulse`. Every check runs before the first change.
8a. The sshd change cannot lock the admin out: after writing the drop-in,
   `sshd -t`; on failure the drop-in is removed and `pair` stops without a
   reload. After a clean check and a reload, `pair` opens a second admin
   connection; if that fails, the drop-in is removed and sshd reloaded again
   through the session `pair` still holds.
9. Refuses (exit 2, nothing changed) when: `uname` is not Linux, `sudo` is
   unavailable, `pulse`'s `authorized_keys` holds any other line, the alias
   name exists in `~/.ssh/config` with a different `HostName`, the machine
   already has a clone with a remote under `<workdir>`, or `PULSE_AGENT` is
   set and no decision names this executor (13).
10. `--install-copilot` installs Node.js 22 and `@github/copilot` on apt
    systems; elsewhere it prints the manual steps and continues without it.
11. Writes one `Host <executor>` block (HostName, IdentityFile,
    IdentitiesOnly) to `~/.ssh/config` and the host key obtained through
    the admin connection to `~/.ssh/known_hosts`.
12. Runs `executor add <executor> --ssh pulse@<executor>` and `executor
    probe`; prints the probe note. `--dry-run` prints every remote command
    and every local file change, and changes nothing anywhere.
13. Who may run it: a human by default. An agent (`PULSE_AGENT` set) only
    when the store holds a human `decide yes`, at most seven days old, on a
    proposal whose body carries `pair: <executor>` (or `onboard: <slug>` for
    `onboard`); the command looks it up before any change, refuses without
    it, and logs the decision's post id in its event. One decision, one
    name, no delegation; spent when the executor is removed or the seven
    days pass. The copilot login on the machine is never part of it.

### Measurement

14. One recorded query (PLAN-021 Verification) gives the time from the
    `project.add` event to the first `done` run with `commits > 0`; no
    dashboard change in this cut (a read-only Flywheel view is a later Tier 1
    work unit under ADR-015).

## Scope

In: the two commands, the planner rule, the decision lookup, docs, tests,
adopter sync.
Out: any dashboard change (ADR-015/016; the Flywheel view is a follow-up),
federated stores (ADR-020 option 2), installing copilot on non-apt systems,
provisioning new VMs (the Azure script keeps that), committing or staging
project-owned files, placing any credential, a new authorization command
(the existing `pulse decide` row is the authorization).

## Interface Changes

- `pulse onboard PATH [--verify CMD] [--name N] [--remote URL] [--brain]`
- `pulse pair ADMIN@HOST[:PORT] --name EXECUTOR [--workdir /srv/pulse]
  [--workers 1] [--projects '*'] [--install-copilot] [--dry-run]`

## Data Changes

None to the schema. New job `source` prefixes: `onboard-reflect:<slug>`.
Events: `project.onboard`, `executor.pair` (with the remote steps' outcome
and, for an agent-run command, the decision's post id; never an address).

## Decisions

[ADR-021](../decisions/ADR-021-top-down-onboarding.md); rests on ADR-019
(store, daemon, `init`), ADR-020 (executors, `pulse-hands`, the probe),
ADR-015/016 (read-only dashboard).

## Edge Cases

- `onboard` on a checkout that is not a git repository: refusal before any
  write (`init` already requires one).
- `docs/evolution/` exists with only some files: refusal before any write,
  the missing names listed for the owner.
- The working tree is dirty elsewhere: not a refusal; `onboard` adds only
  untracked files under `docs/evolution/` and never touches the index.
- `PULSE_AGENT` is set and the newest matching decision is `no`, older than
  seven days, or names a different executor: refusal before any change, the
  reason printed, an event logged.
- `sshd -t` rejects the drop-in (an unusual distro layout): drop-in removed,
  no reload, `pair` stops and prints the complaint; the admin's session and
  access are as before.
- The daemon is not running: jobs queue anyway; the hint names `pulse
  service install`.
- A project onboarded with plain `init` before this change: the planner rule
  gives it its first reflect on the next loop.
- `pair` against a machine that cloud-init already prepared: every step
  finds its state and changes nothing; `add` refuses a duplicate name and
  `pair` reports the existing executor instead.
- The admin session has no `sudo` or asks for a password: refusal with the
  reason; nothing was changed (the checks run before the first change).
- The remote `sshd` reload fails: the block file is removed again and the
  failure printed; the admin's own access was never touched.
- `--install-copilot` on a non-apt system: prints the manual steps, the
  probe later reports `copilot missing`, the machine stays `drained`.
- Address change of a paired machine: the human edits the alias's
  `HostName`; the store never held the address.
- The measurement query on a project with no kept run yet: an empty
  `first_kept`, never an error.

## Rollback Strategy

Per PLAN-021: `git revert` of each step's commit; `executor remove` for a
paired machine plus the admin's removal of the `pulse` user and block (the
inverse is what `--dry-run` prints); the alias block deleted; the scratch
project's scaffold deleted with its checkout. No schema change, nothing
pushed, no project file committed.

## Generated Prompts

None yet; the plan's steps are the brief. A prompt is written when the
implementation is handed to a worker.

## Acceptance Criteria

Those of [PLAN-021](../plans/PLAN-021-top-down-onboarding.md), plus: every
Edge Cases line above has a test or a recorded manual check.

## Verification Plan

`python3 docs/scripts/tests/test_pulse.py` (onboard incl. the partial-folder
refusal, planner rule, pair through the ssh shim with a golden remote
script, the `sshd -t` failure path, every refusal, the decision lookup),
`check-links.sh`, `git diff --check`, one scratch onboarding and one
`pair --dry-run` against the prepared machine recorded in PLAN-021 step 5.
