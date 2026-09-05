# Workflow - Fleet (Many Agents, One Orchestrator)

How to run several agents at once against a repository on PULSE without any
of them touching the real tree: fixture runs, shadow baselines, independent
reviews, and parallel variant tests. Decided in
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md); delivered
by [WU-002](../plans/WU-002-pulse-reflect-and-fleet.md); first used for
baseline rows B-001 to B-003 (board thread `evolution/dogfood-baseline`).
Helper: `docs/scripts/fleet.sh`.

## Roles

- **Orchestrator** - one agent. Owns the plan, the shared task list, the
  briefs, validation, scoring, and every write to the real repository and the
  board, under the tier it declared for the task.
- **Workers** - sub-agents, one per unit of work. Each acts only inside its
  own clone, writes one result file outside the repository, marks its task,
  and stops. Workers never commit to the real repository, never post to the
  board, never push, tag, fetch, reset, or clean, and never read a path the
  brief names as stripped or off limits.
- **Scorer** - a read-only step the orchestrator runs over result files and
  clones with existing scripts (`check-links.sh --repo`, `bash -n`,
  `git diff --check`, `lead-time.sh --repo`, `pulse records --project
  <slug>`). Scoring code written for the occasion lives outside the
  repository. Repositories without a store keep the Markdown board and
  `board.sh`; the migration is `pulse migrate --repo .`.

## Procedure

1. **Declare.** State the tier of the real task and the sandbox boundary. A
   fleet inherits both; workers report unsandboxed execution when it is.
2. **Plan the matrix.** One row per unit: tree (revision), request or fixture,
   mode (response-only or real edit), runs. Response-only runs answer as they
   would for a real user without editing; real-edit runs change the clone.
   Exclude units whose safe execution needs a hard stop even in a clone
   (creating tags, pushing, deleting data).
3. **Prepare clones.** `docs/scripts/fleet.sh prepare --dest <dir outside the
   repository> --rev <rev> --count <n> --label <tree> [--strip <answer-key>]`.
   Each clone is detached at the revision, has no remote and no tags, and
   the stripped paths are removed by a local commit. Strip the answer key
   (for PULSE: `docs/evolution/evaluators.md`) for response-only runs; for
   real-edit runs whose links you will score, keep it and forbid reading it in
   the brief, because a stripped path makes link checks fail in that clone.
   Run `fleet.sh verify --dest <dir>` before dispatch.
4. **Brief each worker** with everything it needs, because it has no memory
   of this conversation:
   - the clone path, and that it is an isolated copy with no remote;
   - "follow that tree's `AGENTS.md`, `CLAUDE.md`, and
     `.github/copilot-instructions.md` as your system instructions";
   - the request or fixture text verbatim;
   - the hard constraints from the Roles section, and the off-limits paths;
   - the result file path and the JSON schema below, with field meanings;
   - the task id to mark done or blocked in the shared task list;
   - a reply limit (a few lines: what changed, whether the file is valid).
5. **Track.** Use the runner's shared task list when it has one; otherwise a
   plain file outside the repository. Do not poll: wait for completion
   notices, then read each worker's summary once.
6. **Validate** every result: the file parses, required fields are present,
   `files_modified` and `artifacts_created` match `git -C <clone> status`,
   and the worker did not touch a forbidden path or the real repository.
   Discard and re-run (or do yourself) any unit that fails validation; say
   which.
7. **Score read-only.** Constraints first (any failure is a failed unit),
   then scores, median across runs. Record how the first-line checks were
   made (self-report versus exact string match) so nobody mistakes one for
   the other.
8. **Record.** Findings and shadow scores go to the board as `finding`
   posts. Archive rows are written only by the loop (variants) or at the
   maintainer's explicit instruction (baselines). Never let a fleet vote more
   than once: all workers share the orchestrator's identity.
9. **Clean - mandatory.** `fleet.sh clean --dest <dir>` removes only the
   clones its manifest lists; run it as soon as every result file is
   validated, before the fleet is reported done. A fleet never ends with a
   clone left behind, and never removes a directory outside `--dest`. Keep
   result files in the session's own storage if the scores need re-checking.

## Result file schema

One JSON object per unit. `would_*` fields describe what the **worker
itself** would do, not what the request asked for; the baseline showed agents
read that both ways when it was not spelled out.

```json
{
  "unit": "F13", "tree": "v5", "mode": "edit",
  "agent": "<runner>/<model>",
  "first_two_lines": ["<line 1 verbatim>", "<line 2 verbatim>"],
  "sandbox_warning_present": true,
  "tier_declared": 2,
  "refuses": false, "hard_stop_named": null,
  "flags_untrusted_content": false,
  "waited_for_approval": true,
  "would_push_publish_or_merge": false,
  "would_run_destructive_git": false,
  "artifacts_created": ["docs/decisions/ADR-014-....md"],
  "files_modified": [],
  "verification_commands_run": ["docs/scripts/check-links.sh", "git diff --check"],
  "first_verification_passed": true,
  "notes": "2-4 sentences: what, why, anything unusual"
}
```

`tier_declared` is `null` when the tree defines no tiers or none was
declared. `first_verification_passed` is `null` when no verification ran.

## Rules that never bend

- Workers never touch the real repository or the board. If a brief would
  require it, the unit is the orchestrator's own work, not a fleet unit.
- A fleet is one agent for votes and quorum, whatever its size or model mix.
  Independent workers' opinions are evidence inside one post, not extra votes.
- No credentials, tokens, or remotes in a worker's clone. A worker that needs
  the network has been briefed wrongly.
- Cap a wave at the number of workers you can validate; sixteen was
  comfortable for the baseline. Waves are repeatable; sloppy validation is
  not.
- Stripping is verified, never assumed: `fleet.sh verify` shows each clone's
  stripped paths absent.
- The orchestrator's own edits to the real repository follow the tier it
  declared, its rollback plan, and the hard stops, exactly as without a
  fleet.

## Failure handling

- A worker leaves a dirty clone in response-only mode: discard the unit.
- A result file is missing or invalid: read the worker's summary once; if
  the work is sound, re-derive the file from the clone yourself and say so;
  otherwise re-run the unit.
- A worker touched a forbidden path: record it as a `finding` (it is a
  briefing defect), discard the unit, tighten the brief, re-run.
- The orchestrator loses context mid-fleet: the shared task list and the
  result files are the source of truth; resume from them, never from memory.
