# Template Sync: autoschedule and refresh guidance

This document explains how to auto-run the PULSE template sync check and how
maintainers can refresh when templates drift.

Summary
- Preferred: run the template-sync skill in "check" mode on a schedule (weekly
  or at the start of a work cycle). Keep applies interactive.
- Options: system cron, systemd timer, runner agent cron, or CI job (owners only).

Example (portable script)

Create a small wrapper the runner or a CI job can call. Replace PULSE_CLI with
the path to the repo's pulse CLI entry point (for example, docs/scripts/pulse.py):

#!/bin/sh
# docs/scripts/run-template-sync-check.sh
: "${PULSE_CLI:?Set PULSE_CLI to the pulse CLI script path, e.g. docs/scripts/pulse.py}"
# Run the template-sync skill in check-only mode and exit nonzero when drift found.
# The exact CLI invocation may differ by runner; this wrapper shows intent and
# a single authoritative place to adapt it for different environments.
python3 "$PULSE_CLI" ballot --project amistio-template >/dev/null 2>&1 || true
# Replace the next line with the runner's way to invoke the pulse skill in check mode:
# Example placeholder (edit for your local pulse CLI):
# python3 "$PULSE_CLI" run-skill --skill pulse-template-sync --mode check --project amistio-template

# Exit 0: wrapper ran successfully. Implementers should change the command
# above to the exact pulse CLI invocation their environment supports.
exit 0

Scheduling suggestions

- Cron (simple): add a cron job on a maintenance host or runner:
  0 2 * * 1 cd /path/to/repo && PULSE_CLI=docs/scripts/pulse.py sh docs/scripts/run-template-sync-check.sh

- Systemd timer (reliable): create a service + timer that runs the wrapper weekly.

- Runner/agent cron: schedule the wrapper on the agent runner machine (same as cron example).

- CI (owner decision): CI can run the wrapper in check-only mode and open an issue when
  drift is detected. Do not configure CI to auto-apply template changes without human review.

What to do when drift is reported

1. Inspect the check-only report; it classifies changes as "sync-safe", "review-first",
   or "never sync" following the canonical workflow.
2. For sync-safe updates, either apply them locally (interactive apply mode) or follow
   the apply guidance in docs/workflows/template-sync.md.
3. For review-first files (AGENTS.md, README.md, .github/* rules), merge rule changes by hand
   preserving project-specific content and ask the team to review.

Notes and safety

- Never configure automatic apply for review-first files.
- Keep the wrapper in one place (docs/scripts/) so runners and CI share the same command.
- Record any intentional divergence in docs/memory/ to avoid repeated re-flagging by future syncs.

Further help

If the repository uses a different pulse runner, adapt the wrapper command to the
local runner's invocation style. The wrapper's intent is to centralize scheduling
and make it clear where to change the exact invocation.
