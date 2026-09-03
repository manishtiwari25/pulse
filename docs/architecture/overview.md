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

## Public Documentation

The GitHub Pages site is a dependency-free static portal served from `docs/`
on the default branch. Its homepage introduces PULSE and the dedicated
`docs/skills/index.html` catalog explains the complete portable skill pack.
Both link back to canonical source in the repository.

## Data and State

- Framework knowledge is stored as versioned Markdown.
- Public presentation is stored as HTML, CSS, and SVG.
- Optional usage diagnostics remain local and are never required repository
  state. The old usage ledger is a frozen historical archive.
- Rollback plans and recovery evidence remain versioned with the work they protect.
- Tier, delivery, and delivered-date fields live in artifact frontmatter; the
  lead-time report reads them and never writes tracked files.
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

## Verification Strategy

- Check Markdown links and static-site references.
- Validate SVG/XML and shell syntax.
- Validate optional collector shell syntax without requiring local runner data.
- Run `bash -n docs/scripts/lead-time.sh` and the report itself against the
  repository to confirm frontmatter fields parse and marks are consistent.
- Use `git diff --check` before publishing framework changes.
