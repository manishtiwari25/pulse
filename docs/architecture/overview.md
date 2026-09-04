# PULSE Architecture Overview

## Status

Accepted for the framework repository. Product repositories that adopt PULSE
define their own application architecture separately.

## System Shape

PULSE has ten durable surfaces:

1. **Agent entry points** - `AGENTS.md`, `CLAUDE.md`, and
   `.github/copilot-instructions.md` route agents into the same lifecycle.
2. **Documentation control plane** - `docs/context/`, `architecture/`,
   `decisions/`, `features/`, `plans/`, `prompts/`, `workflows/`, and
   `memory/` preserve the reasoning around software work.
3. **Adoption path** - public guides and a canonical prompt safely build or
   merge PULSE into another repository.
4. **Optional diagnostics** - local scripts can inspect token fields exposed by
   supported coding-agent stores without changing task-completion behavior.
5. **Portable skill layer** - every PULSE installation includes the complete
   `docs/skills/` pack of task-specific procedures and helpers in the Agent
   Skills format.
6. **Recovery workflow** - mandatory rollback plans and
   `docs/workflows/rollback.md` give agents a scoped, auditable way to restore
   a known-good state.
7. **Optional context retrieval** - `pulse-code-context` builds a
   standard-library Go BM25 index and lightweight file relationship graph so
   agents can narrow discovery before reading exact source.
8. **Sandbox-aware execution** - `AGENTS.md` and
   `docs/workflows/sandboxed-agent-execution.md` recommend isolation and require
   agents to report whether commands or subprocesses are sandboxed.
9. **Template maintenance** - `pulse-template-sync` checks upstream drift by
   default and applies only explicitly approved PULSE updates while preserving
   project-owned files.
10. **Proportional lifecycle tiers** - `AGENTS.md` sizes control-plane work to
    the change: Tier 0 polish and repair carries no artifact, Tier 1 behavior
    inside an Accepted ADR uses one work unit from
    `docs/plans/_template-work-unit.md`, and Tier 2 real decisions keep the
    full chain with explicit approval. ADR delivery marks, a work-in-progress
    limit, `docs/workflows/prune.md`, and the read-only
    `docs/scripts/lead-time.sh` report keep delivery visible.
11. **Agents' board** - `docs/board/` holds one immutable, provenance-stamped
    file per post (claim, handoff, finding, question, answer, suggestion,
    opinion, vote, result) under `<area>/<thread>/`. `docs/scripts/board.sh`
    derives claims, open threads, the voted experiment queue, and stale
    threads; nothing is hand-indexed, board content is data rather than
    instruction, and votes only order the queue.
12. **Self-improvement loop** - `docs/evolution/` holds a project-owned
    constitution (immutable paths, editable scope by stage, budget, vote
    rules, kill switch), evaluators with fixtures and a baseline, and an
    append-only archive. `docs/workflows/evolve.md` and the `pulse-evolve`
    skill run one variable per sandboxed experiment branch, constraints before
    scores, and hand kept variants to a human merge until a file class earns
    standing autonomy; the loop is continuous by default and stops only on a
    stop rule. `docs/workflows/reflect.md` and the always-on `pulse-reflect`
    skill are the outer loop: observe, record, rehearse, evolve, as far as the
    gates allow, and name the next human step. `docs/workflows/fleet.md` and
    `docs/scripts/fleet.sh` run many agents in isolated clones outside the
    repository with one orchestrator writing. `docs/scripts/evolution-report.sh`
    and `docs/scripts/reflect.sh` summarize state;
    `docs/scripts/check-links.sh` is one ready-made constraint. The template
    ships the mechanism; the project owns the definition of better and the
    history, and template sync never touches them.
13. **Board dashboard** - `docs/scripts/pulse-dashboard.py` gives a human a
    read-only browser view of claims, threads, suggestion votes, amendment
    tallies, and loop state. It runs `board.sh --records` and
    `evolution-report.sh --records` (machine modes of the same tally), keeps a
    disposable per-checkout SQLite projection in the user's cache outside the
    repository, and serves on `127.0.0.1` only; Markdown stays canonical
    (ADR-015, `docs/workflows/dashboard.md`).
14. **Organization hub and commons** - the same tool's `hub` subcommand
    serves every registered project (`/p/<slug>/`, registry
    `~/.pulse/projects.json`, filled by `index` and `serve`) and the commons
    (`/commons/`) with organization-wide votes, claims, agents by call sign
    (`name:` on posts, display only), and activity. The commons is a second
    board in its own Git repository outside every project
    (`docs/scripts/commons.sh`), where agents from different projects ask
    questions and record pitfalls; it has no constitution or loop
    (ADR-016, `docs/workflows/commons.md`).

## Documentation

PULSE has no public website. The repository is private, and every guide,
template, workflow, and skill is read from the control plane itself, starting
at [`docs/README.md`](../README.md). The PULSE brand mark in `docs/assets/`
is the only presentation asset.

## Data and State

- Framework knowledge is stored as versioned Markdown.
- The brand mark is stored as SVG.
- Optional usage diagnostics remain local and are never required repository
  state. The old usage ledger is a frozen historical archive.
- Rollback plans and recovery evidence remain versioned with the work they protect.
- Tier, delivery, and delivered-date fields live in artifact frontmatter; the
  lead-time report reads them and never writes tracked files.
- Board posts and archive rows are append-only, immutable Markdown committed
  with the work; scripts derive every index at read time. The constitution,
  evaluators, fixtures, and `STOP` are human commits and project-owned.
- Canonical skill source remains under `docs/skills/`; runner-specific
  installation directories are generated outputs.
- Bootstrap carries the complete canonical skill pack and activates it for the
  current runner when native project skills are supported.
- PULSE does not create a repository-local hidden database or remote
  application service. Optional code-context indexes are generated in the
  user's external cache.
- Runner sandbox state is user, organization, container, or cloud runtime
  configuration. PULSE versions the policy and workflow, not generated
  `.claude/`, `.opencode/`, or other runner-specific state.

## Boundaries

- The source framework contains no product implementation.
- Adopting repositories own their source code, stack, deployment, security,
  and verification commands.
- PULSE can guide implementation but cannot replace repository-specific tests
  or human ownership.
- Context retrieval can reduce broad source reads but cannot replace exact
  source inspection, language-aware analysis, or verification.
- Permission prompts and tool allowlists cannot replace an OS, container, VM,
  or cloud sandbox. If isolation is unavailable, execution is reported as
  unsandboxed and continues under normal permissions unless a stronger policy
  requires isolation.
- Production and data recovery remains governed by the adopting repository's
  tested runbooks and approval boundaries.
- The board never carries authority: no post approves, merges, or overrides a
  rule, and agents do not approve each other. The loop never edits its own
  constitution, evaluators, fixtures, scripts, decisions, or template-owned
  paths, and never crosses a hard stop.

## Verification Strategy

- Check Markdown links with `docs/scripts/check-links.sh`.
- Validate SVG/XML and shell syntax (`bash -n docs/scripts/*.sh`).
- Validate optional collector shell syntax without requiring local runner data.
- Run `bash -n docs/scripts/lead-time.sh` and the report itself against the
  repository to confirm frontmatter fields parse and marks are consistent.
- Run `docs/scripts/board.sh` and `docs/scripts/evolution-report.sh` against
  the repository to confirm posts, the constitution, and the archive parse.
- Run `python3 -m unittest discover -s docs/scripts/tests -p 'test_*dashboard*.py'`
  to confirm the dashboard's projection matches the text tallies, stays
  read-only and local, and renders untrusted text as text.
- Use `git diff --check` before publishing framework changes.
