---
kind: question
area: evolution
thread: owner-questions
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 06931512-be59-48f3-9d76-8a80e4c27e8a
date: 2026-09-04T11:46:00Z
branch: main
links: [docs/evolution/archive.md#B-004, docs/evolution/evaluators.md, docs/board/governance/hard-stop-approval-ambiguity/20260904T114500Z-copilot-cli_claude-fable-5.1.md, docs/workflows/evolve.md]
summary: Seventh owner decision - the fixture runner model; `--model claude-sonnet-5` (B-004's model) and every other named model are "not available" in `copilot -p`, only `auto` answers
---

## question

For the owner; the loop does not wait on this (evolve.md step 12).

7. **Which model measures fixtures from now on?** `B-004` was measured
   with `copilot -p --model claude-sonnet-5`. On 2026-09-04 that flag, and
   every other named model probed (`claude-haiku-4.5`, `claude-sonnet-4.5`,
   `claude-opus-4.7`, `gpt-5.4-mini`, `gpt-5.5`, `gemini-3.5-flash`),
   returns `Error: Model "<id>" from --model flag is not available.` Only
   `--model auto` answers. Cycle 2 therefore stopped unmeasured on a
   runner limit outside the constitution (handoff linked above).

   Options, each a human-owned line in `docs/evolution/evaluators.md`:

   - **(a) Hold.** Keep `claude-sonnet-5` as the measurement model; cycles
     resume when the runner offers it again. Costs nothing; the loop idles.
   - **(b) Re-baseline.** Name the new model (or `auto`) as the
     measurement setting, re-measure the parent under it as a new `B-`
     row, point `baseline:` at that row, and only then measure variants.
     Rough cost: ~3,800 credits per tree at 3 x 14 visible + 6 held-out
     runs, so ~7,600 for parent plus first variant, at cycle-1 prices.
   - **(c) Pin nothing.** Record `auto` as the setting and accept that the
     runner may change the model under it between cycles; cheapest, but a
     kept/discarded decision could then be a model change, not the
     variant.

   The loop recommends (b) with a named model when one becomes available,
   otherwise (a). Whichever line the owner writes, the next agent applies
   it without re-asking.
