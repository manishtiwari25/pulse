# Evolution: The Self-Improvement Loop

This folder holds the three parts a repository needs before agents may
improve how they work on it through measured experiments, as decided in
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md):

| File | Owner | Loop may edit | Purpose |
| --- | --- | --- | --- |
| `constitution.md` | humans | never | Hard stops, immutable paths, editable scope by stage, `limits:` switch and budget, vote rules, kill switch |
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

Before the loop is armed - and between cycles once it is - the always-on
[`reflect.md`](../workflows/reflect.md) workflow (`pulse-reflect`,
`docs/scripts/reflect.sh`) observes, records, and rehearses as far as the
gates allow and names the next human step. Parallel fixture runs and shadow
baselines use isolated clones through [`fleet.md`](../workflows/fleet.md)
(`docs/scripts/fleet.sh`); workers never touch this folder or the board.
The loop is continuous by default: `Cycles per day` is `unlimited` unless a
project writes a number, and a plateau changes the hypothesis instead of
stopping (ADR-012 amendment, 2026-09-03). `limits: none` in the constitution
removes every budget limit at once (ADR-012 Amendment 2; see Limits below).

## Kill switch

Create an empty file named `docs/evolution/STOP` to halt every cycle
immediately. `evolution-report.sh`, `reflect.sh`, `pulse-reflect`, and
`pulse-evolve` check for it first. Only a human removes it.

## Limits

One frontmatter line in the constitution decides whether the Budget table
binds:

```yaml
limits: none     # PULSE default: no token, wall-time, cycle, or plateau limit
limits: budget   # the Budget rows bind as written; "unlimited" in a row = no limit
```

With `limits: none` the run ends only on a stop rule: `STOP`, a constitution
violation, a hard stop, an instruction-shaped board post, or a human
interrupt. A plateau changes the hypothesis; an open question for the owner
is posted to the board and the run continues. Writing very large numbers is
not the way to remove a limit - write `limits: none`. `Runs per fixture` and
`Tolerance` are measurement settings, not limits, and apply in both modes.

What `limits: none` never turns off: the hard stops, the immutable paths, the
human merge for classes without standing autonomy, and the kill switch.

Limits a runner sets on itself are outside the constitution. In GitHub
Copilot CLI, `/limits` shows and edits the session's AI-credit soft cap and
`/autopilot` accepts `--max-ai-credits`; other runners have their own. When a
run stops on one of these, the handoff names the runner setting, not the
constitution.

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
