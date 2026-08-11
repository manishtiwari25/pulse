# Memory

Memory files store durable patterns, mistakes, product rules, and lessons learned for future agents.

PULSE keeps framework memory intentionally small. Repositories that adopt
PULSE should populate memory only after real product work creates a durable
lesson or convention.

## Current State

No project memory has been recorded yet. The files in this folder are placeholders and rules for future population.

## Population Rules

- Add memory only for durable information future agents should reuse.
- Do not store temporary task notes, todos, implementation plans, or active decisions here.
- Use `docs/plans/` for work plans, `docs/decisions/` for ADRs, `docs/features/` for behavior specs, and `docs/context/` for current product or stack facts.
- Keep entries brief, specific, and easy to delete when they become stale.
- Prefer focused files before creating new memory files.
- Include the source of truth when useful, such as an ADR, feature spec, or verified implementation.
- Remove or update memory as soon as it is wrong.

## What Belongs Here

- Repeated coding or workflow patterns that proved useful.
- Product rules that should influence future planning.
- Mistakes, regressions, or integration gaps future agents should avoid.
- Repository conventions that are not already captured better in `AGENTS.md`, `docs/context/`, or `docs/architecture/`.

## What Does Not Belong Here

- Guesses about the future product.
- One-off task status.
- Long explanations or meeting notes.
- Accepted architecture decisions, which belong in `docs/decisions/`.
- Feature requirements, which belong in `docs/features/`.

## Files

- `_template.md` - generic memory note template
- `patterns.md` - reusable patterns discovered during real work
- `mistakes.md` - mistakes to avoid after something breaks or nearly breaks
- `product.md` - accepted product rules and durable preferences
