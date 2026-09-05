# Context

PULSE product and implementation context lives here. Agents read these files
before planning framework or adoption work.

## Current Files

- `product.md` - PULSE users, problem, boundaries, and first outcome.
- `stack.md` - the small static stack used by this framework repository.
- `pulse-store.md` - the machine-level SQLite store where agents read and
  write operating data (projects, agents, posts, jobs, runs, decisions,
  settings, events); rules stay files.

Repositories that adopt PULSE replace these framework facts with their own
product and stack context.
