# Evolution: The Self-Improvement Loop

This folder holds the three parts a repository needs before agents may
improve **the product it ships** - and, second, how they work on it - through
measured experiments, as decided in
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md):

| File | Owner | Loop may edit | Purpose |
| --- | --- | --- | --- |
| `constitution.md` | humans, or agents by ratified amendment | never as a variant | Subject, hard stops, immutable paths, editable scope by stage, `limits:` switch and budget, branches-and-worktrees rule, vote rules, amendment rules and log, kill switch |
| `evaluators.md` | humans | never (may propose) | What "better" means: product constraints and scores first, then operating-layer constraints and scores, fixtures, held-out set |
| `archive.md` | the loop | append only | Every variant tried, kept or discarded, with scores and votes |

The templates `_template-constitution.md`, `_template-evaluators.md`, and
`_template-archive.md` are template-owned and synced from PULSE. The filled
files are project-owned and never synced. Bootstrap installs the templates
but does not turn the loop on; a human fills the constitution and writes the
first evaluators (Stage 1).

Procedure: [`docs/workflows/evolve.md`](../workflows/evolve.md). Skill:
`pulse-evolve`. Read-only report: `docs/scripts/evolution-report.sh`; the
same gates, scope, and archive appear in the browser through the read-only
[board dashboard](../workflows/dashboard.md) (`pulse-dashboard.py serve`).
Suggestions and votes are rows in the `pulse` store, read with `pulse board`
and written with `pulse post` and `pulse vote`; the archive remains this
folder's append-only Markdown file.

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

## Fast owner setup

After the constitution, evaluator suite, and baseline exist, the agent must
not invent the final hidden tests it will later be measured against. The
owner writes only those two private fixture bodies; the helper handles their
ids and both repository gate lines:

```bash
docs/scripts/evolution-setup.sh prepare --store /private/path
# Fill H01.md and H02.md in that private directory.
docs/scripts/evolution-setup.sh activate --store /private/path
```

`prepare` never overwrites a fixture. `activate` validates the private files
without printing their contents, requires a recorded baseline and an absent
`STOP`, then sets `Held out: H01, H02` and `loop: on`. It does not commit;
the owner reviews and commits those two human-owned gate files. The private
path is chosen by the owner, stays outside the repository, and is never
written into tracked files.

## The product first

In a repository that adopts PULSE the loop's subject is the product that
repository ships: its code, tests, and behavior, under Tier 0 (a repair with
a reproducing test, polish with its check) and Tier 1 (behavior inside an
Accepted ADR, the work unit in the same variant), measured by the project's
own checks. The operating layer - memory, project prompts, local rules,
project skills and workflows - is the secondary subject, tried only when no
product hypothesis is eligible or a product signal's cause lives there. Only
in the PULSE source repository is the framework prose the product (ADR-012
Amendment 4, 2026-09-03).

A product class is editable from the first cycle after the owner lists it;
nothing has to be earned first except autonomy. `evolution-report.sh` and
`reflect.sh` print a **PRODUCT SCOPE** verdict; when it says the loop is
confined to the operating layer, the owner's three edits are:

1. **Editable scope:** list the product's paths as `product polish` (Tier 0)
   and `product behavior` (Tier 1) rows at **Stage 4**, `autonomy: human`.
2. **Immutable paths:** keep only the product's hard-stop surfaces there -
   auth and identity, billing, data stores and migrations, deploy and
   infrastructure configuration, secrets, dependency manifests and lockfiles.
   A row such as `src/` parks the loop on the docs; remove it.
3. **Evaluators:** fill the `## Product evaluators` section from
   `_template-evaluators.md` - the repository's existing verification command
   as a constraint, the reproducing-test and work-unit rules, at least one
   product score the project already measures - and record a baseline row.

What the loop then does: picks a product hypothesis before an operating-layer
one, from failing or flaky tests, type and lint findings, coverage gaps,
accessibility and performance reports, `TODO`/`FIXME` markers, unmet
acceptance criteria inside an Accepted ADR, and bug reports or error logs the
project collects; carries the tier's artifact in the variant; keeps the change
only when the product scores say so; and hands every kept product variant to
a human merge until the class earns standing autonomy. Tier 2 surfaces never
enter the loop, in any repository.

## Branches and worktrees

Every experiment branch, worktree, or fleet clone the loop creates is removed
when its cycle ends - the Clean step of
[`evolve.md`](../workflows/evolve.md): a discarded variant's branch and
worktree in the same cycle, a kept variant's branch when its pull request
merges or closes, fleet clones with `fleet.sh clean`. A run never ends with a
stray `evolve/*` branch or worktree; `evolution-report.sh` and `reflect.sh`
list any that exist under **HYGIENE**. The loop removes only what it created,
never by reset, checkout, clean, or force-push.

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

## Amending the constitution

Agents can change the constitution without waiting for a human, but only by
a vote big enough that no single agent or session can carry it
([ADR-014](../decisions/ADR-014-constitutional-amendment-by-supermajority.md),
[`amend.md`](../workflows/amend.md)):

1. An `amendment` post in the store quotes the exact current text and the
   exact proposed text, says why, and says what it does not change.
2. Votes come from distinct top-level sessions - the proposer's is excluded,
   a fleet is one voter - and count only when they carry `text_hash` from
   `pulse hash <post-id>` and a `Read:` line restating the change. That is how
   every voter proves it voted on that text and no other.
3. `pulse records` is the store tally. Defaults: at least forty counted
   voters and two thirds of them in favour; a project may raise both in the
   constitution's Vote rules, and only a human may lower them. Repositories
   without a store keep the Markdown board and `board.sh`; the migration is
   `pulse migrate --repo .`.
4. When the tally says `passed`, any agent applies the proposed text verbatim,
   appends a row to the constitution's `## Amendments` table, commits that
   file alone, and closes the thread with `resolution: ratified`.

Hard stops, the kill switch, the Amendments section, and the frontmatter are
never on the ballot; `STOP` pauses ratification; a human commit outranks any
vote and may revert a ratification. The constitution stays project-owned and
unsynced; PULSE only ships the rows, the tally, and the procedure.

## Stages

Stages say what a file class has open and who merges its kept variants. They
are not an order to climb: a product class the owner lists at Stage 4 is
editable at once; what every class earns through clean cycles is autonomy.

| Stage | Meaning |
| --- | --- |
| 0 | Manual learning; no constitution or evaluators yet |
| 1 | Constitution filled, evaluators written, baseline recorded; board open; a class at Stage 1 is listed but not yet editable |
| 2 | Operating-layer class editable while `loop: on`; a human merges or discards every variant |
| 3 | Standing autonomy for a class that earned it through clean cycles; any incident takes it back |
| 4 | Product class editable while `loop: on` and the product evaluators have a baseline - Tier 0 and Tier 1, the primary subject; Tier 2 never |

The stage per file class lives in the constitution's editable-scope table and
is reported by `evolution-report.sh`, with a PRODUCT SCOPE verdict.
