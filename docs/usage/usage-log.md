# Token Usage Log

Per-session PULSE work-accounting ledger. Each row records the real token data
the active runner exposes. Never guess a missing token field.

- Record input, output, cache, and total tokens when available.
- If a runner exposes only one token field during a live session, record that
  field and label the snapshot `live`.
- Use `n/a (not exposed)` when the runner does not store tokens locally.
- Token counters can keep changing while a session is open. Mark an interim
  row with `†` and the capture time, then finalize it after session shutdown
  when canonical totals are available.
- Run [`docs/scripts/usage.sh`](../scripts/usage.sh) to read supported local
  runner logs.

| Date | Session | Model(s) | Tokens used | Turns | Summary |
| --- | --- | --- | --- | --- | --- |
| 2026-06-27 | a1b2c3d4 | claude-opus-4 (GitHub Copilot) | 12,480 out | 7 | Add the original work-accounting rule, ledger, and script. |
| 2026-07-11 | 0a4a45c8 | claude-fable-5 (Claude Code) | 37,576 out | 44 | Fix usage scripts, add runner collectors, and move operational docs under `docs/`. |
| 2026-07-11 | 15d5dbb1 | claude-fable-5 (Claude Code) | 8,032 out | 7 | Add the template-sync workflow and route it through the agent guide. |
| 2026-07-17 | 27d05029 | claude-fable-5 (Claude Code) | 9,074 out | 15 | Move runner entry points under `docs/` and forbid hidden tool artifacts. |
| 2026-08-07 | c3c19417 | claude-fable-5 (Claude Code) | 18,228 out | 25 | Add the shared ELI5 output format and wire it into agent instructions. |
| 2026-08-08 | 92360e0a | claude-fable-5, gpt-5.6-sol (GitHub Copilot) | 14,902,301 out | 16 | Polish the universal bootstrap guides and retire the duplicate orchestrator flow. |
| 2026-08-08 | 40882170 | gpt-5.6-sol, gpt-5-mini (GitHub Copilot) | 14,154 out | n/a | Coordinate and verify the one-prompt bootstrap guides. |
| 2026-08-11 | 39c58803 | gpt-5-mini, gpt-5.6-sol (GitHub Copilot) | n/a in / 150,262 out / n/a total @ 2026-08-13 13:36 CEST † | n/a | Rebrand and publish PULSE; add the shared audit trail, token-only accounting, agent-driven rollbacks, portable skills, and token-aware delegation advice. |

Older rows contain the output count that was preserved from the original
ledger. New rows should use the fuller token breakdown when the runner exposes it.

† Interim, timestamped snapshot; finalize after the session closes when
canonical totals become available.
