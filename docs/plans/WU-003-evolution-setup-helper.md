---
id: WU-003
title: Two-Command Evolution Setup Helper
status: Done
tier: 1
date: 2026-09-04
delivered: 2026-09-04
controlling_adr: ADR-012
areas: [agents, evolution, workflows]
tags: [work-unit, evolution, evaluators, setup, held-out]
---

# Work Unit: Two-Command Evolution Setup Helper

The evolution loop correctly keeps its held-out fixtures away from the agent
under test, but its current handoff makes the maintainer invent fixture ids,
guess a private format, and edit two gate files manually. This work keeps the
human-owned evidence boundary and removes the avoidable setup work.

## Tier Declaration

- **Tier:** 1 - this packages setup behavior already required by ADR-012 into
  a helper; it does not change who owns evaluators or what opens the loop.
- **Controlling ADR:** [ADR-012: Give Adopting Repositories a Measured, Constitution-Bounded Self-Improvement Loop](../decisions/ADR-012-recursive-self-improvement-loop.md)
- **Why this stays inside the ADR:** ADR-012 already requires two held-out
  tier fixtures outside the repository, a recorded baseline, `loop: on`, and
  human ownership. The helper only assigns ids, scaffolds a private format,
  validates readiness, and applies the two existing gate values after an
  explicit maintainer command.
- **Hard stops touched:** none. The helper does not commit, push, publish,
  merge, remove `STOP`, create evaluator content, or choose a private storage
  location for the maintainer.

## Outcome

After the baseline exists, a maintainer can finish evolution setup with:

```bash
docs/scripts/evolution-setup.sh prepare --store /private/path
# Fill H01.md and H02.md without showing them to the evolving agent.
docs/scripts/evolution-setup.sh activate --store /private/path
```

The helper assigns `H01` and `H02`, validates the private files without
printing their contents, changes only `Held out:` and `loop:`, and leaves the
result for human review and commit.

## Behavior

### User Story

As a repository owner, I want the framework to handle safe evolution wiring
so that my only irreducible setup task is writing two hidden test cases.

### Requirements

- [x] `status` reports gate readiness and the exact next command without
      changing repository or external state.
- [x] `prepare --store PATH` creates private `H01.md` and `H02.md` templates
      outside the repository, never overwrites either file, and prints no
      fixture contents.
- [x] `activate --store PATH` validates both fixtures, their restrictive file
      permissions, the recorded baseline, absent `STOP`, and unique gate
      lines before changing anything.
- [x] Activation changes only `Held out: H01, H02` and `loop: on`; it is
      idempotent, does not commit, and cannot leave a one-file partial update.
- [x] Reflection names the helper instead of asking the owner to invent ids.
- [x] Bootstrap carries the helper to adopting repositories.

### Interface Changes

- New command: `docs/scripts/evolution-setup.sh status|prepare|activate`.
- `prepare` and `activate` require `--store PATH`; PULSE does not select or
  record the maintainer's private location.
- Private fixture ids are fixed as `H01` and `H02`. Each file has one-line
  `tier`, `request`, `expected`, and `checks` fields.

### Data Changes

No repository schema or new PULSE-owned store. The maintainer explicitly
chooses an external directory. `prepare` creates two private Markdown files
there; `activate` writes only the two existing repository gate lines.

### Edge Cases

- The resolved store is inside the repository, including through a symlink:
  refuse before creating or changing anything.
- One private file already exists: preserve it and create only the missing
  file; never overwrite content.
- A field is missing, still `TODO`, duplicated, or has an invalid tier:
  refuse without displaying the value.
- Group or other users can read a fixture: refuse and name the permission
  problem without changing it.
- The baseline is pending or absent from the archive, `STOP` exists, or a
  gate line is missing or duplicated: refuse before either gate changes.
- Activation is repeated after success: report ready and make no extra edit.
- `Held out:` already lists ids the maintainer wrote by hand (not `H01, H02`):
  `status` reports the gate as active and `activate` refuses rather than
  overwriting the maintainer's list.

## Steps

- [x] Add the dependency-free setup helper and focused temporary-repository
      tests.
- [x] Point reflection and evaluator setup guidance at the helper.
- [x] Include the helper in bootstrap inventories and the work-unit index.
- [x] Run focused tests and all existing documentation/script checks.
- [x] Close the work unit and board thread with delivery evidence.

## Implementation Prompt

- **Repository and boundary:** PULSE framework repository; ADR-012's setup
  mechanism only. Evaluator definitions and hidden fixture content remain
  human-owned.
- **Files likely to change:** the files listed in board claim
  `evolution/evolution-setup-helper`.
- **Patterns to reuse:** `docs/scripts/reflect.sh` argument parsing and
  repository discovery; `docs/scripts/fleet.sh` resolved-path safety checks;
  `docs/scripts/tests/test_pulse_dashboard.py` temporary repository tests.
- **Constraints:** Bash with existing system tools only; no fixture content
  in output; no default private location; no commit or remote action; preserve
  unrelated working-tree edits.
- **Do not:** fabricate held-out prompts, weaken the held-out requirement,
  remove `STOP`, write an archive baseline, or expose the external path in a
  tracked file.

## Rollback Plan

- **Baseline:** commit `5935ff4` on `main`, with pre-existing user edits left
  unstaged. Affected surfaces are this work unit, the new helper and test, and
  narrow setup references in evolution, reflection, and bootstrap docs.
- **Trigger:** a focused test, shell syntax, link, whitespace, or lead-time
  check fails; activation can partially update gates; fixture contents appear
  in output; or an existing user edit is overwritten.
- **Reversal:** apply a precise inverse patch for this work unit's additions
  and remove only its new helper/test/work-unit files. Board posts remain
  immutable and receive a closing `dropped` post if delivery is abandoned.
- **State safety:** tests use temporary repositories and stores. The real
  evaluator, constitution, archive, hidden fixtures, dependencies, services,
  and remote state are not changed by verification.
- **Recovery verification:** focused tests no longer reference the helper;
  `docs/scripts/check-links.sh --quiet`, `bash -n docs/scripts/*.sh`,
  `git diff --check`, and `docs/scripts/lead-time.sh` restore the baseline
  results.

## Acceptance Criteria

- [x] The owner writes only two private fixture bodies; ids and gate wiring
      are handled by the helper.
- [x] Fixture contents never appear in command output or tracked files.
- [x] Failure before activation leaves both gate files byte-identical.
- [x] The work stays inside ADR-012 and passes the repository's checks.
- [x] The rollback plan remains scoped and executable.

## Verification

Run on 2026-09-04 against the working tree at `5935ff4`:

- `python3 -m unittest discover -s docs/scripts/tests -p 'test_evolution_setup.py'`
  - 11 tests, OK. Covers: read-only `status` with the exact next command;
  `prepare` preserving an existing file and setting `0600`; refusal of a
  store inside the repository, including through a symlink, before anything
  is created; `activate` refusing unfilled fields, loose permissions, a
  duplicate gate line, `STOP`, a missing baseline row, a symlinked gate file,
  and an existing non-helper held-out set - each with both gate files
  byte-identical afterwards; a successful activation that changes only the
  two gate lines, prints no fixture text, and is idempotent.
- `python3 -m unittest discover -s docs/scripts/tests -p 'test_*.py'` - 34
  tests, OK (the dashboard suite still passes alongside).
- `bash -n docs/scripts/*.sh`, `docs/scripts/check-links.sh --quiet`,
  `git diff --check`, `docs/scripts/lead-time.sh` (WU-003 listed),
  `docs/scripts/board.sh --area evolution` - all exit 0.
- `docs/scripts/reflect.sh --area evolution` - Level C; the NEXT HUMAN STEP
  now names `evolution-setup.sh prepare` instead of asking for hand-written
  ids.
- `docs/scripts/evolution-setup.sh status --repo .` on this repository -
  exit 1 with `Next: docs/scripts/evolution-setup.sh prepare --store /private/path`.
- First test run found one message defect: an untouched template reported
  "invalid tier" instead of "not filled". Fixed before the docs advertised
  the interface.

## Closure

- [x] Set `status: Done` and `delivered: 2026-09-04`.
- [x] ADR-012 remains `delivered`; no decision mark changes.
- [x] One durable pattern recorded in `docs/memory/patterns.md`: automate a
      human gate down to its irreducible part.
- [x] Board thread `evolution/evolution-setup-helper` closed with
      `resolution: promoted`. Nothing committed by the agent: the owner's
      in-flight edits touch the same files, so the owner reviews `git diff`
      and commits (`docs/scripts/` is a CODEOWNERS path).
