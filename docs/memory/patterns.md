# Memory - Patterns

- `2026-08-11` - A Copilot CLI event log can contain `session.resume` after
  `session.shutdown`. An optional token diagnostic must ignore the earlier
  shutdown snapshot when later activity exists. Apply when parsing append-only
  runner session logs.
- `2026-08-20` - A repository context index is a discovery map, not the source
  of truth. Apply when: use cached chunks and graph edges to narrow reads, then
  verify the exact source before planning or editing.
- `2026-08-22` - Permission controls are not a sandbox. Apply when: distinguish
  approval controls from an OS, container, VM, or cloud isolation boundary.
- `2026-08-22` - Safety warnings should lead to the next safe action. Apply
  when: include the active harness's official sandbox documentation directly
  in the warning, with the local PULSE workflow as the fallback.
- `2026-08-27` - A safety recommendation should not pretend to be a hard gate.
  Apply when: prefer sandboxing, disclose unsandboxed execution clearly, and
  let stronger organization or task policies decide when isolation is
  mandatory.
- `2026-08-27` - Template maintenance should inspect before it mutates. Apply
  when: make sync checks read-only for tracked files and state, and require an
  explicit apply request before changing an adopted repository.
- `2026-08-27` - Private runner telemetry should not become a completion gate.
  Apply when: keep usage diagnostics opt-in, avoid repository writes, and use
  provider-supported systems for formal reporting.

## Populate When

- A coding, testing, planning, or review pattern proves useful more than once.
- A repository convention should guide future implementation.
- A workflow rule is not already captured better in `AGENTS.md` or `docs/workflows/`.

## Entry Format

- `YYYY-MM-DD` - Short pattern. Apply when: concise trigger.
