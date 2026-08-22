# Memory - Patterns

- `2026-08-11` - A Copilot CLI event log can contain `session.resume` after
  `session.shutdown`. A token collector must ignore the earlier shutdown
  snapshot when later activity exists. Apply when parsing append-only runner
  session logs.
- `2026-08-20` - A repository context index is a discovery map, not the source
  of truth. Apply when: use cached chunks and graph edges to narrow reads, then
  verify the exact source before planning or editing.
- `2026-08-22` - Permission controls are not a sandbox. Apply when: require a
  verified OS, container, VM, or cloud isolation boundary for agent execution,
  and fail closed instead of retrying outside it.
- `2026-08-22` - Safety warnings should lead to the next safe action. Apply
  when: include the active harness's official sandbox documentation directly
  in the warning, with the local PULSE workflow as the fallback.

## Populate When

- A coding, testing, planning, or review pattern proves useful more than once.
- A repository convention should guide future implementation.
- A workflow rule is not already captured better in `AGENTS.md` or `docs/workflows/`.

## Entry Format

- `YYYY-MM-DD` - Short pattern. Apply when: concise trigger.
