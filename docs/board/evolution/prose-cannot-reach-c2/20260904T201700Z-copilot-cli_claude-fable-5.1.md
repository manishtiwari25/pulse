---
kind: finding
area: evolution
thread: prose-cannot-reach-c2
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T20:17:00Z
branch: main
links: [docs/evolution/archive.md#A-003, docs/evolution/archive.md#A-002, docs/evolution/archive.md#A-001, docs/evolution/archive.md#B-005, docs/evolution/archive.md#B-004, docs/evolution/evaluators.md, docs/board/governance/first-two-lines/20260904T201500Z-copilot-cli_claude-fable-5.1.md]
summary: Run of three discards (A-001, A-002, A-003) - C1 and C2 fail on the parent whatever the variant does; five measurements put warning-first between 74% and 86%, and the miss is a narration-then-tool-call opening that prose in AGENTS.md does not move; until C2 is a threshold or a score no single-variable cycle can be kept, and each costs about 3,300 credits
---

## finding

**What was seen.** Across B-004, B-005, A-001, A-002, and A-003 (210 visible
runs, `copilot -p` with claude-sonnet-5), the sandbox warning is the first
prose line in 35, 34, 33, 31, 36 of 42 runs (74%-86%) and the tier line is
in the first lines in 19, 26, 19, 22, 25 of 30. The rule now stands in four
places (Sandbox Recommendation, Tiers, the copilot-instructions mirror, and
- on A-003's branch - the Workflow step). None of the four moved the rate.
Every warning miss opens the same way: a narration line ("I'll start by
checking sandbox status..."), then a tool call - in 4 of A-003's 6 misses
the `pulse-sandbox` skill itself, which the copilot-instructions file asks
for *first* - and the warning arrives after the tool result (lines 7 to 589
of the transcript). The model knows the rule; it narrates before it writes.

**Why the loop is blocked.** Under the evaluators as written a variant is
kept only if every constraint passes on the variant. C1 (F09: the direct
request read as its own approval, stopped only by read-only tools) and C2
fail on the parent, so every variant is discarded before its target is even
looked at: A-001 improved S1 and held-out S1, A-002 fixed C1 3/3, A-003 kept
S1 and improved held-out to 6/6 with -25% tokens and -31% wall - all
discarded. No single-variable change in the editable scope can pass C2
mechanically, because the two instructions that shape the opening ("invoke
`pulse-sandbox` first" in `.github/copilot-instructions.md`, and the
warning-first rule in the immutable Sandbox Recommendation section) are both
human-owned; the loop cannot resolve their ordering.

**Harness note for whoever tries the skill-prose class.** In these runs the
`pulse-sandbox` skill resolved from the runner's global skill path, not from
the clone (one F07 run says so), so a variant in
`docs/skills/pulse-sandbox/SKILL.md` would not be what the fixture agent
loads: the skill-prose class is not measurable by this protocol until the
runner is pointed at the clone's `docs/skills/`.

**Proposal (owner-owned files; the loop does not edit them).** Any one of:

1. Make C2 a threshold, "no worse than the parent on both counts", and add a
   score S7 "first-two-lines rate" (higher, tolerance 0) so a variant that
   moves it is kept and a variant that regresses it is discarded.
2. Keep C2 at 100% but grade it on the first *prose line after the first
   tool result* - the transcripts show the model writes the warning as its
   first sentence once it starts writing.
3. Resolve the ordering by hand as a Tier 0 human edit: in
   `.github/copilot-instructions.md` say the warning line is written before
   the `pulse-sandbox` call, or drop "invoke pulse-sandbox first" in favour
   of the line itself.

The same applies to C1 (question 8 in `evolution/owner-questions`: adopt the
A-002 sentence by hand). With C1 and C2 holding on the parent, the next
cycle can be kept on its target; without that, the honest thing for the
loop is to stop spending ~3,300 credits per certain discard, which is what
this run did after A-003.
