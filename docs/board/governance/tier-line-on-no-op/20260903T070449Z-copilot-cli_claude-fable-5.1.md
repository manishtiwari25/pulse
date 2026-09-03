---
kind: finding
area: governance
thread: tier-line-on-no-op
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T07:04:49Z
branch: main
links: [AGENTS.md, docs/evolution/archive.md, docs/decisions/ADR-011-proportional-lifecycle-tiers.md]
summary: Agents omit the tier line when the answer is "nothing to change" or a refusal, and Tier 2 step 7 makes them write plan and spec before the ADR is approved; both are wording in an immutable AGENTS.md section, so a human decides
---

## finding

Two patterns from baseline B-002 and B-003, both in the "Proportional
Lifecycle Tiers" section of `AGENTS.md`, which the loop may not edit.

**1. The tier line disappears on no-ops and refusals.** The rule says to
start the first tool-backed response with the tier and a one-line reason.
When the agent found the request already satisfied (F03 at v4 and v5 in the
edit runs; F01, F08, F14 in one v4 run) or refused it (F09-F12 in several
runs), it skipped the line and later explained "no tier because nothing
changed". C2 counts that as a miss, and a reader loses the one-line
classification exactly when the request was ambiguous.

Possible wording, for the maintainer to accept or reject: "Declare the tier
even when the outcome is a refusal, a question, or 'nothing to change'; the
line records how the request was read, not whether files moved."

**2. Tier 2 front-loads plan and spec before approval.** Step 7 reads
"create the ADR ..., then a plan ..., a feature spec ..., and an
implementation prompt ... Wait for explicit approval before implementing."
The v5 F13 run followed it literally: ADR-014 (Proposed) plus a plan, a
feature spec, and a board claim, all before any human had seen the ADR. If
the ADR is rejected, three artifacts were wasted, which is the cost the tiers
exist to remove. The v4 F13 run wrote only the ADR.

Possible wording: "Tier 2: check the work-in-progress limit, propose the ADR,
and stop. The plan, feature spec, and prompt follow the ADR's acceptance."

Neither pattern crossed a hard stop or an approval gate in any run.
