# One-Prompt PULSE Setup

Use the same prompt for a blank repository, a new repository with an initial
README, or an established codebase.

## Copy This Prompt

**Open the target repository in an AI coding agent, paste this prompt, and
approve its proposed file changes.**

<!-- UNIVERSAL_BOOTSTRAP_LAUNCHER_START -->
```text
Bootstrap the repository currently open in this AI coding agent with PULSE -
Planning & Unified Lifecycle for Software Engineering - from
https://github.com/manishtiwari25/pulse.

Use your available GitHub, web, or git tools to discover that source
repository's default branch; do not assume a branch name. From that branch,
read and follow:
- docs/prompts/shared/bootstrap-control-plane-in-new-repo.prompt.md
- docs/workflows/bootstrap-control-plane.md
Read any other source files those instructions require. If the public
repository or either required file cannot be accessed, stop and report the
access problem clearly; do not improvise from memory.

Inspect this target repository before editing and automatically decide whether
it is new/empty or established. Complete the integration end to end through
your normal file-change approval flow. Do not ask me routine setup questions:
infer facts from the repository, mark genuinely unknown facts as open
questions, and continue.

Preserve product/source code, uncommitted work, and existing project-specific
instructions. Merge instead of blindly overwriting. Keep new PULSE
control-plane content under docs/; do not add hidden tool-specific control
folders. Do not copy stale PULSE source context, source-repository usage rows,
generated history, or unrelated repository state. Adapt names, paths, stack
details, and commands to this repository. Include the complete canonical
PULSE skill pack under docs/skills/ and activate all eight for the current
runner when native project skills are supported. Create project-specific ADRs,
specs, or plans only when the evidence makes them useful. Do not commit.

Validate changed Markdown links, inspect the final diff, run git diff --check
and any existing targeted documentation checks, then report files changed,
preserved files, assumptions, validation results, and blockers.
```
<!-- UNIVERSAL_BOOTSTRAP_LAUNCHER_END -->

No placeholders need replacing. The agent discovers PULSE's current default
branch and learns about the target from the files already there.

## What the Agent Does

1. Loads the [canonical PULSE bootstrap prompt](shared/bootstrap-control-plane-in-new-repo.prompt.md)
   and [bootstrap workflow](../workflows/bootstrap-control-plane.md).
2. Inspects the target and chooses the safe new-repository or
   established-repository merge path.
3. Builds or merges the PULSE `docs/` control plane, including all eight
   canonical skills, without changing product code.
4. Activates the complete pack for the current runner when supported.
5. Validates links and diffs, then reports changes and blockers.

If the agent cannot access the public source or required canonical files, it
stops instead of guessing. Enable source access, then paste the same prompt
again.
