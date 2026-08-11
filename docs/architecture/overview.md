# PULSE Architecture Overview

## Status

Accepted for the framework repository. Product repositories that adopt PULSE
define their own application architecture separately.

## System Shape

PULSE has five durable surfaces:

1. **Agent entry points** - `AGENTS.md`, `CLAUDE.md`, and
   `.github/copilot-instructions.md` route agents into the same lifecycle.
2. **Documentation control plane** - `docs/context/`, `architecture/`,
   `decisions/`, `features/`, `plans/`, `prompts/`, `workflows/`, and
   `memory/` preserve the reasoning around software work.
3. **Adoption path** - public guides and a canonical prompt safely build or
   merge PULSE into another repository.
4. **Observability helpers** - local scripts read real token counts from
   supported coding-agent session stores and record them in `docs/usage/`.
5. **Recovery workflow** - mandatory rollback plans and
   `docs/workflows/rollback.md` give agents a scoped, auditable way to restore
   a known-good state.

## Public Documentation

The GitHub Pages site is a dependency-free static portal served from `docs/`
on the default branch. It introduces PULSE, explains the lifecycle, and links
to the canonical Markdown source in the repository.

## Data and State

- Framework knowledge is stored as versioned Markdown.
- Public presentation is stored as HTML, CSS, and SVG.
- Token usage remains local until an agent appends a session snapshot to the
  committed usage ledger.
- Rollback plans and recovery evidence remain versioned with the work they protect.
- PULSE does not create a hidden database or remote application service.

## Boundaries

- The source framework contains no product implementation.
- Adopting repositories own their source code, stack, deployment, security,
  and verification commands.
- PULSE can guide implementation but cannot replace repository-specific tests
  or human ownership.
- Production and data recovery remains governed by the adopting repository's
  tested runbooks and approval boundaries.

## Verification Strategy

- Check Markdown links and static-site references.
- Validate SVG/XML and shell syntax.
- Run token collectors against available local runner logs.
- Use `git diff --check` before publishing framework changes.
