# PULSE Prompts

Reusable prompt templates and future generated implementation prompts live here.

## Rules

- Keep prompts specific, actionable, and testable.
- Include likely files, requirements, acceptance criteria, and verification commands when those are known.
- Keep prompts model-agnostic.
- Do not create shell scripts to batch-run prompts.

## Folders

- `shared/` - shared prompt templates and reusable workflows

## Generated Implementation Prompts

- [`project-board-observability-dashboard.prompt.md`](project-board-observability-dashboard.prompt.md) -
  the implementation prompt that built the local read-only board and voting
  dashboard (ADR-015, delivered 2026-09-04); reuse it for a rebuild or a port.

## Setup

- [`integration-prompts.md`](integration-prompts.md) - one copy-and-paste
  PULSE launcher for a new, empty, or established repository
