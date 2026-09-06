---
id: PLAN-021
title: Top-Down PULSE - `pulse onboard`, `pulse pair`, the First Reflect Now
status: In Progress
tier: 2
date: 2026-09-06
delivered:
tags: [plan, runner, onboarding, pairing, executors, daemon, agent-first]
---

# Plan: Top-Down PULSE - `pulse onboard`, `pulse pair`, the First Reflect Now

## Tier

Tier 2 - two new commands, one of which configures another machine, and a
planner rule; no ADR covered them until
[ADR-021](../decisions/ADR-021-top-down-onboarding.md). Waits for the
human's `pulse decide` on the board proposal before step 1.

## Goal

A fresh project reaches its first kept commit after one command and two
named human commits (the evaluators' baseline and `loop: on`), and an
existing Linux machine becomes a pair of hands after one command - while the
daemon stays the single supervisor and the dashboard stays read-only.

## Context

- `docs/scripts/pulse.py`: `init` (register + onboarding job), `plan_jobs`
  (improve -> answer -> one reflect a day), `add_executor`, `probe_executor`,
  `ssh_command`/`ssh_run` (argv in base64, `BatchMode=yes`), the daemon's
  slot filling, `install_service`.
- `docs/scripts/hosted/pulse-hands.sh` (the forced command),
  `cloud-init.yaml` (the same hardening, applied at first boot - the source
  of truth for what `pair` must reproduce), `azure-provision.sh`.
- `docs/evolution/_template-constitution.md`, `_template-evaluators.md`,
  `_template-archive.md` (the scaffold `onboard` copies; `loop: off`).
- `docs/scripts/pulse-dashboard.py` is not touched by this plan (ADR-021
  section 4); the Flywheel view is a later Tier 1 work unit under ADR-015.
- Tests: `docs/scripts/tests/test_pulse.py` (ssh shim, `HANDS_SH`,
  `ExecutorTestCase`), `test_pulse_dashboard.py`,
  `test_pulse_dashboard_guards.py` (immutable).
- Constraints: project-owned files are never committed or staged by the
  command; `pair` and `onboard` are the human's unless the store holds a
  `decide yes` on a proposal naming the target (`pair: <executor>` /
  `onboard: <slug>`, seven days; ADR-021 section 3); no address in the store
  or on the board (ssh alias); every remote step idempotent, every check
  before the first change, anything unexpected a refusal; `sshd -t` before
  any reload and a second admin connection before `pair` lets go.

## Steps

- [x] **Step 1 - `pulse onboard`.** New subcommand wrapping `init`: scaffold
      `docs/evolution/` from the templates only when the folder is entirely
      absent; a partial folder is a refusal (exit 2, missing names listed)
      before any write; queue the onboarding job and a `reflect` job
      (priority 19, source `onboard-reflect:<slug>`, one ever per project)
      directly - not through the planner; print the new paths
      (`git status --short docs/evolution`), the two human gates, and the
      `pulse service install` hint when the daemon is not alive; when
      `PULSE_AGENT` is set, require the `onboard: <slug>` decision (section
      3) first. Tests: scaffold created once, partial folder refused with
      nothing written, existing files untouched, both jobs queued, index
      unchanged, `init` behavior unchanged, agent without decision refused.
- [x] **Step 2 - planner rule.** `plan_jobs` may plan a `reflect` job for a
      project that has never had one, regardless of the daily cadence, so a
      project onboarded with plain `init` also starts within one loop.
      Test: fresh project, empty board, first `plan_jobs` yields a reflect.
- [x] **Step 3 - `pulse pair`.** New subcommand: hub key generation, the
      remote steps of ADR-021 section 2 as one idempotent script sent over
      the admin session (`sudo` required; `--dry-run` prints every remote
      command and changes nothing), `~/.ssh/config` alias, host key trusted
      through the admin connection, `executor add`, `probe`. All checks run
      before the first change; the sshd block is a drop-in followed by
      `sshd -t` (failure: drop-in removed, no reload, stop), then a reload,
      then a second admin connection (failure: drop-in removed and reloaded
      through the held session). `--install-copilot` only on apt systems.
      Refusals: an `authorized_keys` for `pulse` with any other line, a
      missing `sudo`, a non-Linux uname, an alias name already in
      `~/.ssh/config` pointing elsewhere, `PULSE_AGENT` set without a
      `pair: <executor>` decision in the last seven days. Tests through the
      ssh shim: the generated remote script is byte-stable (golden), every
      refusal fires with nothing written, the `sshd -t` failure path removes
      the drop-in, `add` + `probe` run, the alias is written once, the event
      carries the decision id.
- [x] **Step 3b - `pulse executor login` (ADR-021 Amendment 1).** New
      `executor` action: the token arrives on standard input only, its prefix
      is checked before anything is sent (`github_pat_`, `gho_`, `ghu_`
      pass; `ghp_` and everything else refuse with exit 2), it is piped over
      the admin session into `sudo -u pulse -H copilot login --with-token`,
      the remote output is passed through with token-shaped text redacted,
      and `probe` follows; `--remove` deletes `~pulse/.copilot` and probes.
      One `executor.login` event with the name, token kind, and exit code -
      never the token or the admin address. In `pulse-hands`, `logged_in` is
      `yes` only when `~/.copilot/config.json` records a login (or the CLI's
      token variable is set). Before the token is sent, the consent
      `storeTokenPlaintext: true` is written to `~pulse/.copilot/settings.json`
      (a headless machine has no keychain and the CLI answers its own
      consent prompt with no when standard input is a pipe).
      Tests through an admin-session shim: refusals send nothing, the remote
      commands are byte-stable, the consent script runs for real against a
      temporary home (write, present, merge), a failed consent step keeps
      the token home, output is redacted, the event never carries the token,
      the probe rule distinguishes `logs/`-only and `settings.json`-only
      folders from a real login record.
- [x] **Step 4 - docs and sync.** `docs/workflows/pulse.md` (onboard),
      `docs/workflows/executors.md` (pair beside the Azure path; the "Never"
      paragraph gains the decided-proposal exception), ADR-020 section 6
      amendment note, `docs/context/pulse-store.md` (job sources, the
      decision lookup), `AGENTS.md`, `README.md`, Copilot and Claude
      instruction files; sync to adopters; ADR-021 `delivery: partially
      delivered` after step 1 lands, `delivered` here.
- [ ] **Step 5 - prove it, cheaply.** Onboard one scratch project on this
      machine (`onboard` -> baseline -> `loop: on`) and record the time to
      its first kept commit with the query below; run `pair --dry-run`
      against the machine cloud-init already prepared and record that every
      step reports present and nothing changed. No new VM is created for
      this plan.

## Progress

- 2026-09-06: steps 1-4 landed on the brain in one fleet pass (four workers in
  isolated clones, merged after `python3 -m unittest discover -s
  docs/scripts/tests` - 181 tests - links, `bash -n`, `git diff --check`).
  Step 5, first half: `pulse onboard` on a scratch checkout under a temporary
  `PULSE_HOME` registered the project, scaffolded `docs/evolution/` with
  `loop: off`, queued the onboarding and first-reflect jobs, staged nothing,
  and printed the two gates; the second run was a no-op. `pulse pair
  pulseadmin@pulse-vm1 --name pulse-vm1 --dry-run` against the machine
  cloud-init prepared returned exit 0 with every inspection field present
  (`uname=Linux sudo=yes user=present hands=differs workdir=present
  authorized_keys=ours dropin=present dropin_dir=yes remotes=0 apt=yes
  copilot=present node=present git=present gitconfig=ok`; `hands=differs`
  because the machine still runs the pre-Amendment-1 hands script) and
  changed nothing on either side. `pulse executor login` refused an empty
  standard input and a classic `ghp_` token before sending anything (exit 2,
  no event). Open: the time-to-first-kept-commit measurement waits for the
  first real onboarding with `loop: on`; the login of `pulse-vm1` waits for a
  fine-grained token with the Copilot Requests permission.
- 2026-09-06, later: `pulse-vm1` activated. The maintainer handed a
  fine-grained token by password-manager reference; the first
  `pulse executor login pulse-vm1 --admin pulseadmin@pulse-vm1 --with-token`
  validated it ("Login succeeded") and stored nothing - the CLI's
  plaintext-storage consent has no terminal to ask on a piped login. The
  consent (`storeTokenPlaintext: true` in `~pulse/.copilot/settings.json`)
  was written, the second run stored the login (`config.json` gained
  `authTokens` and `loggedInUsers`), the probe answered `logged_in=yes`, and
  the executor returned from `drained` to `active`. The command now writes
  that consent itself, and the hands count a login only from the
  `config.json` record, so the consent file alone cannot pass as one. The
  machine still runs the earlier hands script (`hands=differs`) until a real
  `pulse pair` applies the new one.

## Rollback Plan

- **Baseline:** brain `main` at the commit before step 1; the live store
  `~/.pulse/pulse.sqlite` (schema 2); `~/.ssh/config` without new `Host`
  blocks; no scratch project registered.
- **Trigger:** any failure in `python3 -m unittest discover -s
  docs/scripts/tests`; `pair` leaving a remote `authorized_keys` line without
  `command=`/`restrict`, or an sshd that refuses the admin; `onboard`
  touching an existing `docs/evolution/` file or staging anything; an
  agent-run `pair`/`onboard` proceeding without its decision row; or the
  human's no.
- **Reversal:** `git revert` of the step's commit(s) on the brain and the
  adopters' sync commits; `pulse executor remove` for an executor `pair`
  added; the remote machine's `pulse` user, its `authorized_keys`, and the
  `Match User pulse` block removed by the admin (the exact inverse is printed
  by `pair --dry-run`); the `Host` alias block deleted from `~/.ssh/config`;
  a scratch project's `docs/evolution/` scaffold deleted with the scratch
  checkout.
- **State safety:** the store gains job rows with new `source` prefixes
  only - no schema change; project-owned files are written only where
  absent and never committed; `pair` never touches a machine's existing
  users, keys, or sshd defaults outside the `pulse` user and its `Match`
  block; nothing is pushed.
- **Recovery verification:** both test suites pass; `pulse executor list`
  shows only the executors that existed before; the admin can still ssh to
  the machine; `pulse status` unchanged.

## Acceptance Criteria

- [x] `pulse onboard` on a fresh checkout: project registered, scaffold
      present and uncommitted, onboarding + reflect jobs queued, the two
      human gates printed; a second run changes nothing.
- [x] A project's first `reflect` is planned within one daemon loop of
      onboarding (store evidence: the job's `created_at`).
- [x] `pulse pair --dry-run` prints every remote command and changes
      nothing; against the machine cloud-init prepared it reports every step
      present; the shim tests cover the `sshd -t` failure path and every
      refusal; `PULSE_AGENT` set without a decision is refused.
- [ ] Time from onboard to first kept commit is recorded for one scratch
      project with the query below; the human steps between were exactly
      two.
- [ ] Relevant control-plane files are updated and synced; on closure,
      `delivered:` is set here and ADR-021's `delivery:` mark is updated.

## Verification

```bash
python3 -m unittest discover -s docs/scripts/tests    # test_pulse, test_pulse_onboard, test_pulse_pair, test_pulse_login, the dashboard suites
bash -n docs/scripts/hosted/pulse-hands.sh docs/scripts/hosted/azure-provision.sh
bash docs/scripts/check-links.sh && git diff --check
python3 docs/scripts/pulse.py onboard /tmp/scratch-project --verify 'true'    # then: baseline, loop: on, watch pulse status
python3 docs/scripts/pulse.py pair admin@host --name hands-2 --dry-run
python3 docs/scripts/pulse.py executor login hands-2 --admin admin@host --with-token < /dev/null   # refuses: no token, nothing sent
sqlite3 ~/.pulse/pulse.sqlite "SELECT e.at AS onboarded, MIN(r.finished_at) AS first_kept
  FROM events e JOIN runs r ON r.project = e.subject
  WHERE e.action = 'project.add' AND e.subject = '<slug>' AND r.status = 'done' AND r.commits > 0"   # ADR-021 section 5
```

- Walk every Edge Cases line of
  [FEAT-014](../features/014-top-down-onboarding.md) against the code before
  ticking Acceptance Criteria.
