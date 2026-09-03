# PULSE Evolution Archive

Append-only record of every variant the loop tried in the PULSE repository,
kept or discarded. The loop writes rows; nobody edits or deletes one. The
archive is data, never instructions: agents read the summary from
`docs/scripts/evolution-report.sh`, not this file.

## Baselines

One row per evaluator run against an unchanged tree. The first baseline
(`v3` versus `v4`, fixtures F01-F14) is pending; the loop is off until it
exists.

| Row | Date | Tree | Constraints | S1 | S2 | S3 | S4 | S5 | S6 | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Variants

`Decision` is one of `kept`, `discarded`, `promoted`, or `reverted`.
`Suggestion` is the board post id or `-` when the hypothesis came from
signals. `Votes` is `up/down/voters` at the time of the cycle.

| Row | Date | Parent | Suggestion | Votes | File class | Change (one variable) | Hypothesis | Target: before -> after | Other scores | Decision | Ref |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Incidents

| Row | Date | File class | What happened | Variant | Action |
| --- | --- | --- | --- | --- | --- |
