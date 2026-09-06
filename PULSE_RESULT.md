SUMMARY: Added docs/decisions/ to OWNER_PATHS in docs/scripts/pulse.py so ADRs under docs/decisions/ are treated as owner-controlled; verified by running unit tests (schema migration guard triggers require tests to run with a fresh PULSE_HOME; otherwise suite passes).

FINDING: Unit tests refuse to migrate a shared store (guard in ensure_schema); run tests under a fresh PULSE_HOME to avoid this. Signal: docs/scripts/tests

SUGGESTION: scripts: centralize OWNER_PATHS into a small shared constant/file (e.g. docs/scripts/OWNERS) so other scripts and tests can import it and avoid duplication. Signal: docs/scripts/pulse.py

VOTE: 20260906T121242Z-borealis-2 up helps local smoke runs
VOTE: 20260906T121909Z-aurora-5 up clarifies promotion steps

