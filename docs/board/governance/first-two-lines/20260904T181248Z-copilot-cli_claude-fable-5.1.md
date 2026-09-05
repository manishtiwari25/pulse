---
kind: claim
area: governance
thread: first-two-lines
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T18:12:48Z
branch: main
links: [docs/board/governance/first-two-lines/20260904T133000Z-copilot-cli_claude-fable-5.1.md, docs/evolution/archive.md#B-005, docs/evolution/evaluators.md, ADR-012]
summary: Claim - evolve cycle 3 runs the first-two-lines card now; parent is B-005 (tree b3bd739) because the stripped fixture-visible tree of HEAD 4068c8b is identical to it, so only the variant is measured (42 visible runs + 6 held-out, ~3,500 credits); unsandboxed.
expires: 2026-09-05T02:00:00Z
files: [AGENTS.md, docs/plans/WU-007-first-two-lines.md, docs/evolution/archive.md, docs/board/governance/first-two-lines/]
---

**Cycle.** Evolve cycle 3, Tier 1 under this repository's acceptance of
ADR-012, file class "operating guide prose". One variable: the sentence in
the card above, added to `AGENTS.md` Workflow step 4 on
`evolve/operating-guide-prose/first-two-lines` (worktree outside the main
tree), with `docs/plans/WU-007-first-two-lines.md` as the Tier 1 artifact.

**Parent decision.** The default branch moved from `b3bd739` (B-005) to
`4068c8b`: four board posts in stripped threads (`owner-questions`,
`hard-stop-approval-ambiguity`, this card), `archive.md` (stubbed in every
clone), and one evolve lesson in `docs/memory/patterns.md` that names archive
rows - the same class of loop-internal lesson the fleet already strips from
`mistakes.md`, and a string the leak check already rejects. With those strips
the fixture-visible trees are byte-identical (checked with `diff -r` on one
prepared clone of each), so B-005 stays the parent and only the variant is
measured. Any difference would have meant a new `B-006` wave instead.

**Boundary.** Unsandboxed - Copilot CLI normal permissions, no `/sandbox`
active; the fixture runs happen in stripped detached clones under
`~/.pulse/fleet/c3-*`, never in this tree. Recorded in the archive row.

**Protocol.** As B-005/A-002: `copilot -p` with `claude-sonnet-5`, 14 fixtures
x 3 runs, read-only tools for F09-F12, blind held-out H01/H02 x 3 with only
counts read. Constraints before scores; C2 is the target (warning first
34/42 -> 42/42, tier line early 26/30 -> 30/30); S1 tolerance 0, others 5%.

**Ends with.** Archive row A-003, a `result` here with `resolution: tested`,
`fleet.sh clean` for every `c3-*` fleet, worktree removed, branch deleted if
discarded or handed to the owner for a pull request if kept (`gh` cannot
reach the remote from this machine, and the experiment commit is unsigned
because the owner's 1Password signing key is not available to an agent).
