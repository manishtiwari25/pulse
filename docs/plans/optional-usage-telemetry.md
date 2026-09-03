---
id: PLAN-009
title: Make Usage Telemetry Optional
status: Done
tier: 2
date: 2026-08-27
delivered: 2026-08-28
tags: [plan, usage, telemetry, agents, delegation]
---

# Plan: Make Usage Telemetry Optional

## Goal

Remove mandatory token snapshots, response footers, and per-session repository
writes. Keep the local collectors as opt-in diagnostics, freeze the existing
ledger as historical evidence, and let the delegation advisor work without a
history file by default.

## Context

- Runner session formats are private, change without notice, and may be hidden
  by the active sandbox.
- Several runners expose no trustworthy local token total, so mandatory output
  produces `n/a` instead of useful evidence.
- Task completion must not depend on usage telemetry.
- Formal cost or organization reporting belongs in provider-supported APIs,
  outside the repository workflow.

## Steps

- [x] Add a decision that supersedes mandatory Work Accounting.
- [x] Remove required footers and ledger writes from active instructions.
- [x] Reframe retained collectors as optional, best-effort diagnostics.
- [x] Remove collectors that can only report missing token data.
- [x] Make delegation history an explicit optional input.
- [x] Freeze the existing usage ledger and update public/control-plane docs.
- [x] Validate instructions, scripts, advisor tests, links, HTML, and diff
  safety.

## Rollback Plan

- **Baseline:** clean `main` at
  `47aa1610ba625266198ff647f54051fd807b1ebe`, with mandatory Work Accounting,
  per-session writes to `docs/usage/usage-log.md`, and eleven canonical skills.
- **Trigger:** active instructions still require snapshots, optional collectors
  are presented as authoritative, the delegation advisor fails without
  history, links break, skill discovery changes, or repository checks fail.
- **Reversal:** apply a precise inverse patch only to files changed by this
  plan, restore the two removed no-token collectors, and remove ADR-010 and
  this plan.
- **State safety:** do not run collectors against user data, change provider
  settings, install dependencies, alter external services, or rewrite existing
  usage rows.
- **Recovery verification:** confirm the original Work Accounting sections,
  ledger guidance, advisor default, scripts, links, and eleven-skill discovery
  are restored, then run `git diff --check`.

## Acceptance Criteria

- [x] No active agent instruction requires a Work Accounting footer.
- [x] No active agent instruction requires repository usage-log writes.
- [x] Active policy does not create an `n/a` footer or new ledger row for
  missing token data.
- [x] Retained collectors are opt-in and explicitly best effort.
- [x] The delegation advisor runs with no history argument.
- [x] An explicit compatible history file still improves advisor estimates.
- [x] The historical ledger is marked frozen and is not part of task workflow.
- [x] Current documentation and public pages describe the new policy.
- [x] Verification is complete and recorded.

## Verification

Run:

```bash
bash -n docs/scripts/usage*.sh
python3 -m unittest discover -s docs/skills/pulse-delegation-advisor/tests
python3 docs/skills/pulse-delegation-advisor/estimate_tokens.py \
  --task "Update optional usage guidance" \
  --files 4 \
  --risk low \
  --novelty low \
  --ambiguity low \
  --reversibility easy
/Library/Developer/CommandLineTools/usr/bin/git diff --check
```

Also validate active-policy searches, Markdown links, public HTML, eleven-skill
metadata, and universal skill discovery.

## Result

Completed with:

- no mandatory Work Accounting or token-footer rule in active instructions
- no normal-work writes to the frozen usage archive
- four retained opt-in collectors with valid shell syntax and no `n/a`
  placeholder output
- removal of the VS Code and Cursor collectors that could not return token
  counts
- eight passing delegation-advisor tests
- successful advisor checks with no history and with explicit compatible
  history
- valid Markdown links and parseable public HTML
- universal discovery of all eleven skills
- clean `git diff --check` output and no tracked runner-generated directories

The universal installer completed successfully while warning that the local
Node.js `22.18.0` is below its declared `22.20.0` minimum.
