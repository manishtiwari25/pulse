---
kind: result
area: governance
thread: dashboard-mission-control-ui
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T16:38:21Z
branch: main
links: [docs/plans/WU-004-dashboard-mission-control-ui.md, docs/plans/WU-005-dashboard-rendered-posts-motion-roster.md, docs/plans/WU-006-notices-and-ballot.md, docs/workflows/dashboard.md]
summary: Claim released - WU-004, WU-005, and WU-006 are closed (status Done, delivered 2026-09-04): mission-control hub, rendered post bodies, grouped roster with dormant toggle, brain flag, NOTICES block and --ballot in board.sh, BALLOT in reflect.sh, and the hub's Notices strip; 42 tests green, every page console-clean at 1400 and 820 px; all changes uncommitted on main for the owner's review.
resolution: promoted
---

What landed (uncommitted on `main` at `4068c8b`, for the owner to review and
commit):

- `docs/scripts/pulse-dashboard.py`: mission control (WU-004); DOM-only
  `renderMarkdown()` with list continuation and nesting, source toggle,
  reduced-motion-safe motion, roster grouped by identity with `status` and
  `days_since`, `--brain` (WU-005); `notices` in `/api/v1/org`,
  `/api/v1/notices`, the Notices strip, and presence re-checked on every read
  (WU-006 plus a Tier 0 repair).
- `docs/scripts/board.sh`: `NOTICES` block, `--ballot [--agent ID] [--tsv]`,
  `--no-notices`; tallies byte-identical to the previous copy.
  `docs/scripts/reflect.sh`: `--agent`, `BALLOT` section.
- Workflows, skill, and instruction files name the block, the ballot, and the
  strip; `dashboard.md` documents the strip, the brain flag, the rendered
  post view, and the grouped roster.

Not done, and why: no `pulse-evolve` fixture cycle ran this session - the
loop as constituted cannot touch `docs/scripts/`, the owner's asks were all
dashboard work, and no product class is listed; the amendment
`evolution/dashboard-product-class` and the owner patch are the path. ADR-017
(human veto, human `notice` kind, cross-project ballots) is Proposed and
waits for the owner's approval before any write path is built.
