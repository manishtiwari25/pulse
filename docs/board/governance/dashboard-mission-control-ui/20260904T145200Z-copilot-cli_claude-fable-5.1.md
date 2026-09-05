---
kind: finding
area: governance
thread: dashboard-mission-control-ui
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T14:52:00Z
branch: main
links: [docs/scripts/pulse-dashboard.py, docs/scripts/reflect.sh, docs/memory/mistakes.md, docs/plans/WU-004-dashboard-mission-control-ui.md]
summary: reflect's whitespace check caught two trailing spaces inside the embedded APP_JS that the 37 tests, `node --check`, and the browser all accept; run `git diff --check` before restarting the server. Fixed on main; no strays.
---

What happened: `docs/scripts/reflect.sh --area governance` reported
`whitespace: FAILED - git diff --check` after WU-004 was closed and verified.
Both lines were `fill(x, ` with a trailing space inside the `APP_JS` string
constant of `docs/scripts/pulse-dashboard.py`. Nothing else saw them: the
unit tests read the assets as text and never check whitespace, `node --check`
parses fine, and the browser does not care.

Why it matters: whitespace is a constraint in the loop's health checks, so an
experiment variant carrying the same slip would have been discarded on a
constraint, not a score.

What I did: stripped trailing whitespace in the scratch assets, re-spliced,
re-ran `node --check`, `git diff --check`, and the 37 tests (all clean), and
restarted the hub on 127.0.0.1:8765 so it serves the fixed constant.

For the next agent: after any edit to `INDEX_HTML`, `APP_CSS`, or `APP_JS`,
run `git diff --check` before the tests and before restarting the server.
The full lesson (text checks do not execute the front-end) is now in
`docs/memory/mistakes.md`. Hygiene: this session created no branch,
worktree, or clone; the two test servers it started (ports 8767 and the
earlier hub on 8765) are stopped.
