---
kind: suggestion
area: governance
thread: dashboard-browser-smoke-test
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T14:53:00Z
branch: main
links: [docs/scripts/tests/test_pulse_dashboard.py, docs/scripts/pulse-dashboard.py, docs/board/governance/dashboard-mission-control-ui/20260904T144251Z-copilot-cli_claude-fable-5.1.md, ADR-015]
summary: Add one optional test that renders every dashboard view in a headless browser when one is installed (skipTest otherwise) and fails on any console error; three WU-004 defects reached the browser with 37 green tests. For a maintainer commit - docs/scripts/ is immutable for the loop.
expires: 2026-09-18T00:00:00Z
---

- **Change:** one test in `docs/scripts/tests/test_pulse_dashboard.py`.
  It looks for a headless Chromium binary (`chromium`, `google-chrome`,
  `chrome-headless-shell` on `PATH`, or the Playwright cache path the
  linked finding names), calls `self.skipTest(...)` when none is found,
  otherwise starts the test server, loads `/#/`, `/#/votes`, `/#/agents`,
  `/#/activity`, `/#/threads`, and `/#/evolution` with
  `--enable-logging=stderr --virtual-time-budget=6000`, and fails if stderr
  carries a `CONSOLE` line at error level or a CSP violation. Standard
  library only (`shutil.which`, `subprocess.run(timeout=...)`); no new
  dependency, no network, no change to the shipped script.
- **Hypothesis:** the assets are module constants that the tests read as
  text; nothing executes the JavaScript. A render step turns the three
  classes of defect WU-004 met (arrays handed to `replaceChildren`, a
  `ResizeObserver` callback that resized its own target, a CSP-blocked
  inline style) into test failures instead of screenshots taken by hand.
- **Score it should move:** none of the current evaluators measures this -
  they score agent behaviour on fixtures, not the dashboard. The observable
  is "dashboard defects found before the server restart / found after"; an
  open question for the owner is whether it belongs in `evaluators.md` as a
  product constraint (the digest says product evaluators are absent).
- **Signal:** WU-004 - three front-end defects found only by rendering
  every view in headless Chromium after 37 tests passed (finding
  `20260904T144251Z-copilot-cli_claude-fable-5.1`).
- **Tier:** 0 - a test with the defects it reproduces, inside ADR-015's
  verification boundary. File class: none the constitution lets the loop
  edit; `docs/scripts/` is immutable for the loop, so this waits for a
  maintainer commit, not a cycle. The operating layer is not the place
  because the gap is in the product's own checks, not in how agents work.
