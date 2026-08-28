# Archived Token Usage Log

> Frozen on 2026-08-27 by
> [ADR-010](../decisions/ADR-010-optional-usage-telemetry.md). Do not append or
> update rows during normal work.

These rows preserve the history of the retired Work Accounting experiment.
They are incomplete, use different token bases, and are not authoritative
telemetry or billing data. Optional local diagnostics remain under
[`docs/scripts/`](../scripts/).

| Date | Session | Model(s) | Tokens used | Turns | Summary |
| --- | --- | --- | --- | --- | --- |
| 2026-06-27 | a1b2c3d4 | claude-opus-4 (GitHub Copilot) | 12,480 out | 7 | Add the original work-accounting rule, ledger, and script. |
| 2026-07-11 | 0a4a45c8 | claude-fable-5 (Claude Code) | 37,576 out | 44 | Fix usage scripts, add runner collectors, and move operational docs under `docs/`. |
| 2026-07-11 | 15d5dbb1 | claude-fable-5 (Claude Code) | 8,032 out | 7 | Add the template-sync workflow and route it through the agent guide. |
| 2026-07-17 | 27d05029 | claude-fable-5 (Claude Code) | 9,074 out | 15 | Move runner entry points under `docs/` and forbid hidden tool artifacts. |
| 2026-08-07 | c3c19417 | claude-fable-5 (Claude Code) | 18,228 out | 25 | Add the shared ELI5 output format and wire it into agent instructions. |
| 2026-08-08 | 92360e0a | claude-fable-5, gpt-5.6-sol (GitHub Copilot) | 14,902,301 out | 16 | Polish the universal bootstrap guides and retire the duplicate orchestrator flow. |
| 2026-08-08 | 40882170 | gpt-5.6-sol, gpt-5-mini (GitHub Copilot) | 14,154 out | n/a | Coordinate and verify the one-prompt bootstrap guides. |
| 2026-08-11 | 39c58803 | gpt-5-mini, gpt-5.6-sol (GitHub Copilot) | n/a in / 202,794 out / n/a total @ 2026-08-14 06:41 CEST † | n/a | Rebrand and publish PULSE; add the shared audit trail, token-only accounting, rollbacks, portable skills, delegation advice, and the default full-pack skills catalog. |
| 2026-08-20 | c3a5138e | gpt-5.6-sol, gpt-5.4 (GitHub Copilot) | 52,995,109 in / 449,036 out / 53,444,145 total @ 2026-08-27 10:44 UTC † | 387 main + 34 subagent requests | Add standard-library Go code-context search and a relationship graph, evolve sandboxing into a visible recommendation, package safe template synchronization, and retire mandatory usage snapshots. |

Rows marked `†` were interim snapshots that may differ from final provider
totals. Git history preserves the original operating rules.
