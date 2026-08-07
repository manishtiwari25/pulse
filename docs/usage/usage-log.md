# Usage Log

Per-session work-accounting ledger. Each row records the **real usage reported by the runner** for a session — never a guess from a static price table.

- **Cost is the runner's native unit:** AIC (GitHub Copilot CLI), USD (OpenCode / paid APIs), or token cost. Record the value shown by that runner's own usage view.
- **Figures are cumulative, live counters.** Token counts and credit/AIC/USD totals keep climbing while a session runs. Record the value at **session close**; any row added mid-session is an **interim, timestamped snapshot** (mark it with `†` and the time).
- **Never fabricate.** If the runtime does not expose an exact figure, record what the runner's usage view shows and label unreadable values `≈ estimate`.
- Run [`docs/scripts/usage.sh`](../scripts/usage.sh) to read real usage from every harness's local logs (Claude Code, Copilot CLI, OpenCode, Codex — CLI and IDE entry points of the same product share the same logs). Subscription runners that expose no USD are recorded as `n/a (plan)`.
- When a Copilot CLI session **closes**, its event log records the canonical totals: real input/output/cache tokens, AI units (`totalNanoAiu` ÷ 1e9 ≈ AIC), and premium-request count. For a **live** session those aren't finalized yet — sum main + subagent output and paste the AIC from the status line (`Session: N AIC used`). See [`docs/scripts/usage-copilot.sh`](../scripts/usage-copilot.sh).

| Date       | Session  | Model(s)                       | Cost (unit)           | Output tokens | Turns | Summary                                                  |
| ---------- | -------- | ------------------------------ | --------------------- | ------------- | ----- | -------------------------------------------------------- |
| 2026-06-27 | a1b2c3d4 | claude-opus-4 (GitHub Copilot) | ~5 AIC used @ 14:32 † | 12,480        | 7     | Example row — add Work Accounting rule, ledger & script. |
| 2026-07-11 | 0a4a45c8 | claude-fable-5 (Claude Code)   | n/a (plan) @ 14:02 †  | 37,576        | 44    | Fix usage scripts, add codex/vscode/cursor collectors + unified usage.sh, move scripts/ & usage/ under docs/. |
| 2026-07-11 | 15d5dbb1 | claude-fable-5 (Claude Code)   | n/a (plan) @ 14:20 †  | 8,032         | 7     | Add template-sync workflow + /template-sync Claude skill; wire into AGENTS.md routing. |
| 2026-07-17 | 27d05029 | claude-fable-5 (Claude Code)   | n/a (plan) @ 06:44 †  | 9,074         | 15    | Move template-sync skill from .claude/skills/ into docs/prompts/shared/; add no-artifacts-outside-docs rule to AGENTS.md/CLAUDE.md; ignore tool dirs in .gitignore. |
| 2026-08-07 | c3c19417 | claude-fable-5 (Claude Code)   | n/a (plan) @ 06:45 †  | 18,228        | 25    | Add shared ELI5 output format as the default for all LLM output (messages + artifact prose, any model) with a "How to Switch" guide (docs/prompts/shared/eli5.prompt.md); wire into AGENTS.md, CLAUDE.md, copilot-instructions.md. |

† Interim, timestamped snapshot — counters were still climbing at the recorded time; finalize at session close.
