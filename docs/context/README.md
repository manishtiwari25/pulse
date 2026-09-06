# Context

PULSE product and implementation context lives here. Agents read these files
before planning framework or adoption work.

## Current Files

- `product.md` - PULSE users, problem, boundaries, and first outcome.
- `stack.md` - the small static stack used by this framework repository.
- `pulse-store.md` - the machine-level SQLite store where agents read and
  write operating data (projects, agents, posts, jobs, runs, decisions,
  settings, events); rules stay files.
- `_template-environment.md` - the environment recipe a project adopting
  PULSE fills in as `docs/context/environment.md` (ADR-026): the tool pins,
  setup lines, plain variables, and services a fresh machine needs before the
  project's check can run. `pulse onboard` scaffolds it when absent; the
  runner reads it at every job's base commit.
- `environment.md` - this repository's own recipe (project-owned, never
  synced): no pins, no setup, no services; the test suites are the check.

Repositories that adopt PULSE replace these framework facts with their own
product and stack context.
