---
name: pulse-delegation-advisor
description: Estimate likely token usage with an optional user-supplied history file and recommend agent-led, human-led, or hybrid execution. Use before delegating a task or when comparing agent effort with human work.
---

# PULSE Delegation Advisor

Estimate token usage from task signals and, when explicitly supplied, a
compatible local history file. Then combine the estimate with risk, ambiguity,
novelty, reversibility, and external-state impact.

## Required Behavior

1. Read the task and identify:
   - likely files changed
   - risk
   - novelty
   - ambiguity
   - reversibility
   - external or persisted state
2. Run the included `estimate_tokens.py` script from this skill's directory.
3. Use `--history <path>` only when the user supplies a compatible local
   Markdown history file. Do not inspect runner stores automatically.
4. Report the lower, central, and upper estimate, token basis, confidence,
   recommendation, reasons, and limitations.
5. Recommend:
   - **agent-led** for clear, reversible, low/medium-risk work within budget
   - **hybrid** for high novelty, ambiguity, broad scope, or uncertain estimates
   - **human-led** for critical risk, hard-to-reverse production/data state, or
     when supplied economics clearly favor human work
6. Do not claim which option is cheaper in money unless the user supplies:
   - estimated human minutes
   - human hourly rate
   - agent cost per million tokens
7. Never store supplied history or financial values in the repository.
8. Treat the result as advice, not permission to bypass PULSE rollback,
   security, production, data, or approval rules.

## Example

```bash
python3 estimate_tokens.py \
  --task "Add a documented API endpoint with tests" \
  --files 4 \
  --risk medium \
  --novelty medium \
  --ambiguity low \
  --reversibility easy
```

Optional history:

```bash
python3 estimate_tokens.py \
  --history /path/to/team-token-history.md \
  --task "Add a documented API endpoint with tests"
```

A compatible history file uses a Markdown table with `Date`, `Session`,
`Model(s)`, `Tokens used`, `Turns`, and `Summary` columns. Keep it outside the
repository unless the project intentionally owns that data.

Optional economics:

```bash
python3 estimate_tokens.py \
  --task "Update a small internal report" \
  --human-minutes 45 \
  --human-hourly-rate 80 \
  --agent-cost-per-million-tokens 12
```

## Report

Lead with the recommendation, then show the estimate and confidence. Explain
which task signals and any supplied history records drove the result. Without
compatible history, label the generic estimate low confidence. If economics
inputs are missing, say that tokens alone cannot determine monetary cost.

Decision source:
<https://github.com/manishtiwari25/pulse/blob/main/docs/decisions/ADR-006-token-aware-delegation-advice.md>
