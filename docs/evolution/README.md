# Evolution: The Self-Improvement Loop

This folder holds the three parts a repository needs before agents may
improve how they work on it through measured experiments, as decided in
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md):

| File | Owner | Loop may edit | Purpose |
| --- | --- | --- | --- |
| `constitution.md` | humans | never | Hard stops, immutable paths, editable scope by stage, budget, vote rules, kill switch |
| `evaluators.md` | humans | never (may propose) | What "better" means: pass/fail constraints, optimized scores, fixtures, held-out set |
| `archive.md` | the loop | append only | Every variant tried, kept or discarded, with scores and votes |

The templates `_template-constitution.md`, `_template-evaluators.md`, and
`_template-archive.md` are template-owned and synced from PULSE. The filled
files are project-owned and never synced. Bootstrap installs the templates
but does not turn the loop on; a human fills the constitution and writes the
first evaluators (Stage 1).

Procedure: [`docs/workflows/evolve.md`](../workflows/evolve.md). Skill:
`pulse-evolve`. Read-only report: `docs/scripts/evolution-report.sh`.
Suggestions and votes come from the [agents' board](../board/README.md).

## Kill switch

Create an empty file named `docs/evolution/STOP` to halt every cycle
immediately. `evolution-report.sh` and `pulse-evolve` check for it first.
Only a human removes it.

## Stages

| Stage | Meaning |
| --- | --- |
| 0 | Manual learning; no constitution or evaluators yet |
| 1 | Constitution filled, evaluators written, baseline recorded; board open; loop changes nothing |
| 2 | Supervised cycles over the operating layer; a human merges or discards every variant |
| 3 | Standing autonomy for operating-layer file classes that earned it |
| 4 | Product code under Tier 0 and Tier 1, class by class; Tier 2 never |

The current stage per file class lives in the constitution's editable-scope
table and is reported by `evolution-report.sh`.
