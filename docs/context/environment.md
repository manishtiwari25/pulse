---
recipe: 1
tools: []
env: {}
services: []
suggested_check: python3 -m unittest discover -s docs/scripts/tests
---

# Environment

The PULSE framework repository (the brain) needs nothing a paired machine does
not already have: git and Python 3 for the store, the runner, and its tests.
There is no lockfile at the root, so the WU-021 heuristic installs nothing;
`setup` is left unset on purpose.

- `tools: []` - the scripts under `docs/scripts/` are standard-library Python;
  the dashboard's committed `dist/` needs no build to be served.
- `env: {}` - the tests create their own `PULSE_HOME` under a temporary
  directory and never read the machine's store.
- `services: []` - the store is a SQLite file; no service runs beside a clone.
- `suggested_check` - the same command the store's `verify_cmd` carries for
  this project: the full runner, pair, and dashboard suites (about two
  minutes; the pair suite needs `ssh` on `PATH` and skips when absent).

A future pin (a Node version for a dashboard rebuild, say) belongs in
`tools:`, committed alone, landed by a human ([ADR-026](../decisions/ADR-026-project-environment-recipes.md)).
