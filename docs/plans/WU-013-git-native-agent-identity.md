---
id: WU-013
title: Git-Native Agent Identity - A Key per Run, Trailers per Commit, Verification Before Landing
status: Done
tier: 1
date: 2026-09-06
delivered: 2026-09-06
controlling_adr: ADR-022
areas: [agents, governance, security]
tags: [work-unit, runner, git, identity, signing, executors]
---

# Work Unit: Git-Native Agent Identity

The maintainer accepted ADR-022 on 2026-09-06 (`pulse decide
20260906T072458Z-fable-scale-09051203 yes`). Until now a worker's commit
said *who* only as a config line: `user.name = <agent>` in the clone. This
work unit makes every commit a PULSE worker lands **evidence**: authored by
its agent, annotated with its job and executor, signed by an ed25519 key the
hub made for that run alone, and verified by the hub before it reaches a
`pulse/<agent>/<job>` branch. The human's own commits, merges, and pushes
are untouched.

## Tier Declaration

- **Tier:** 1 - the behavior ADR-022 decided, built as it is written there.
- **Controlling ADR:** [ADR-022: Git-Native Agent Identity](../decisions/ADR-022-git-native-agent-identity.md)
  (Accepted 2026-09-06).
- **Why this stays inside the ADR:** every piece below is named in ADR-022
  §1-§4: the clone configuration, the `prepare-commit-msg` hook, the per-run
  key, the `key` verb on the hands, the landing check, `runs.signing_pubkey`,
  `pulse verify`, and `pulse signers`. No GitHub credential, account, or App
  is involved; the keys can only sign; humans still merge and push.

## Outcome

- A worker commits with plain `git commit` and the commit carries
  `Pulse-Agent`, `Pulse-Job`, `Pulse-Executor` trailers and an SSH signature
  from the run's key; the hub refuses to land a commit that is unsigned,
  signed by another key, authored by another name, or missing its trailers,
  and says which commit and why.
- `pulse verify <ref>` and `pulse signers` let a maintainer audit any landed
  commit from the merged history with git alone.

## Behavior

1. `run_job` makes an ed25519 key (`ssh-keygen -q -t ed25519 -N ''`) per
   run in the hub's work directory, stores the public half on the run row
   (`runs.signing_pubkey`, schema version 3, additive migration), and removes
   the private half with the run.
2. The clone is configured at clone level: `user.name`, `user.email`,
   `gpg.format ssh`, `gpg.ssh.program ssh-keygen` (pinned: never a
   machine-wide signer), `user.signingkey <key>`, `commit.gpgsign true`,
   `tag.gpgsign false`, `core.hooksPath .git/hooks` (pinned: never a
   machine-wide hooks directory), and a `prepare-commit-msg` hook that adds
   the three trailers with `git interpret-trailers --if-exists doNothing`.
3. Remote hands: after `prepare`, the hub calls the new verb
   `key JOB EXECUTOR` with the private key on stdin; the hands write
   `WORKDIR/JOB/run.key` (0600) and configure the clone exactly as in 2;
   `clean JOB` removes it with the directory; `probe` reports `keys=` (job
   directories holding a key) and the hub treats a key left without a live
   run and not swept as a probe problem.
4. Landing: the hub fetches the clone's `HEAD` to a temporary ref, checks
   every commit since the base with `git log --format='%H %G? …'` under a
   one-line allowed-signers file for that run's key, requires `G`, the
   agent as author (`<agent> <<agent>@pulse.local>`), and matching
   `Pulse-Agent` and `Pulse-Job` trailers; on success it creates the branch,
   otherwise it lands nothing and the run fails with a plain reason.
5. `pulse verify <ref> [--project]` resolves the commit's `Pulse-Job` to its
   run(s), verifies the signature against the stored key, prints agent, job,
   run, executor, and the verdict; exit 0 only on `G`. `pulse signers
   [--project]` prints an allowed-signers file for every run the store
   knows.
6. The worker brief's rule 5 says the clone signs with this run's key and
   adds trailers, and that `--no-verify`, `--no-gpg-sign`, `--author`, or
   `-c user.*` makes the hub refuse the commit.

## Steps

- [x] Work unit written; ADR-022 set to Accepted.
- [x] `pulse.py`: schema v3 + migration; `make_run_key`, `provenance_hook`,
      `configure_clone_identity`; `LocalExecutor`/`SshExecutor` prepare and
      cleanup; landing verification; `pulse verify`, `pulse signers`; brief
      line; probe `keys=`.
- [x] `pulse-hands.sh`: `key` verb, `probe` `keys=`, header comment.
- [x] Tests: signed run lands with `G` and trailers (local and remote);
      impersonated author refused; unsigned commit refused; `--no-verify`
      commit refused for its trailers; `verify` and `signers`; the hands'
      `key` verb refuses a non-key; v2 store migrates.
- [x] `docs/context/pulse-store.md` (column, verbs, commands); memory pattern
      for the `gpg.ssh.program` / `core.hooksPath` pins.
- [x] Refresh `pulse-vm1` with `pulse pair` (already accepted for that
      machine); ADR-022 `delivery:`; close this unit.

## Implementation Prompt

Build exactly the Behavior list above in `docs/scripts/pulse.py`,
`docs/scripts/hosted/pulse-hands.sh`, and `docs/scripts/tests/test_pulse.py`.
Keep `pulse.py` standard-library only. Never store a private key in the
store. Never push, never add a remote, never touch the human's global git
configuration. Existing tests whose shell workers commit as `w <w@x>` now
describe impersonation: turn those into plain `git commit` calls and add the
refusal tests separately. Run `python3 -m unittest discover -s
docs/scripts/tests` before committing.

## Rollback Plan

- **Baseline:** `main` at `3cbba34`; files: `docs/scripts/pulse.py`,
  `docs/scripts/hosted/pulse-hands.sh`, `docs/scripts/tests/test_pulse.py`,
  `docs/context/pulse-store.md`, this unit, ADR-022, one memory entry.
- **Trigger:** any test red; a real run that fails the identity check when
  the worker behaved; `pulse pair` refresh failing on `pulse-vm1`.
- **Reversal:** `git revert` of this unit's commits. Clones go back to
  `commit.gpgsign false` and no hook. The `runs.signing_pubkey` column stays
  (additive, ignored by the reverted code, PLAN-020's migration rule);
  already-landed signed commits stay valid history.
- **State safety:** no store rows are rewritten; keys live only in the hub's
  work directory and the hands' job directory and are removed with them; a
  paired machine keeps working with the reverted hub because `prepare`,
  `clean`, and `probe` keep their arity and the `key` verb is simply unused.
- **Recovery verification:** `python3 -m unittest discover -s
  docs/scripts/tests` green; `pulse executor probe pulse-vm1` ok; one local
  `pulse work --job <id>` landing a branch.

## Acceptance Criteria

- [x] A landed branch's commits show `G` under the run's allowed-signers
      line, the agent as author, and the three trailers.
- [x] Impersonated, unsigned, and trailer-less commits land nothing, and the
      run's summary names the commit and the reason.
- [x] `pulse verify` and `pulse signers` work on a landed branch.
- [x] Both executors covered by tests; the full suite green.
- [x] `pulse-vm1` refreshed and probing ok.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests`
- `bash -n docs/scripts/hosted/pulse-hands.sh`
- `python3 docs/scripts/pulse.py executor probe pulse-vm1` after the refresh
- `git diff --check`; `bash docs/scripts/check-links.sh`

## Closure

Set `status: Done` and `delivered:`; ADR-022 `delivery: delivered`.

## Closure

Delivered 2026-09-06. Full suite 199 tests green (1 skipped); `bash -n`
clean. `pulse pair pulseadmin@pulse-vm1 --name pulse-vm1` refreshed the
hands (`step=hands result=done`) and the probe reports `ok ... keys=-`; a
`pulse work --dry-run --executor pulse-vm1` prepared a clone through the new
`key` verb and left no key behind. Before the refresh the daemon - which had
already restarted on the new `pulse.py` - met the old hands twice and ended
both runs `error` with the intended hint (`verb not allowed: key - the hands
predate ADR-022; refresh them ...`), requeuing the job without spend; that
hint now names `<admin@host>` rather than the hands' own alias, which
`pulse pair` cannot use. ADR-022 is `delivered`.
