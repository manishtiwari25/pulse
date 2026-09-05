---
kind: finding
area: governance
thread: dashboard-mission-control-ui
agent: copilot-cli/claude-fable-5.1
name: Northstar
runner: copilot-cli
model: claude-fable-5.1
session: 97cd210a-39ce-4703-827b-54f11ff39d86
date: 2026-09-04T14:42:51Z
branch: main
links: [ADR-015, docs/plans/WU-004-dashboard-mission-control-ui.md, docs/workflows/dashboard.md]
summary: WU-004 done, uncommitted on main for review - three things the next dashboard change would rediscover, found only by rendering the served page in a headless browser, not by the 37 unit tests.
---

**Status.** The mission-control redesign in
`docs/scripts/pulse-dashboard.py` is complete and verified (37 tests, `node
--check`, headless Chromium in both themes at two widths); it sits
uncommitted on `main` with WU-004 closed. The claim above lapses on its
own.

**Finding 1 - the CSP allows CSSOM, not inline style attributes.** The
page's policy is `style-src 'self'`. That blocks every `style="..."`
attribute that a script would set with `setAttribute` (the browser reports
"Refused to apply inline style"), but it does not block `node.style.cssText
= '...'` or `node.style.width = '40%'` from the page's own script, and SVG
presentation attributes (`d`, `x`, `width`) are not styles at all. So bar
widths, series colours, and tooltip positions are written through the
`style` property; `var(--token)` works there but not in SVG presentation
attributes, which is why colours go through `.style.stroke` and never
through a `stroke="var(--x)"` attribute. The one `http://` literal the
script may contain is the SVG namespace identifier that
`document.createElementNS` requires; the test now allows exactly that string
and nothing else, and it is never fetched.

**Finding 2 - `replaceChildren` does not flatten arrays.** Passing
`list.map(...)` as one argument renders the literal text
`[object HTMLDivElement],...`. The unit tests cannot see this because they
never execute the script; only the screenshot did. The page now routes every
child list through one `fill(node, ...kids)` helper that flattens and drops
`null`/`false`, and `el()`/`svg()` do the same for their children.

**Finding 3 - a `ResizeObserver` callback that redraws the observed box
re-enters itself.** Drawing the chart changes the box height, the observer
fires again in the same frame, and Chromium logs "ResizeObserver loop
completed with undelivered notifications". Deferring the redraw with
`requestAnimationFrame` and redrawing only when the width changed ends the
loop.

**How to see the page without a display.** The Playwright cache on this
machine has a headless shell:
`~/Library/Caches/ms-playwright/chromium_headless_shell-*/chrome-headless-shell-mac-arm64/chrome-headless-shell --headless --disable-gpu --virtual-time-budget=6000 --window-size=1400,1400 --screenshot=out.png URL`
prints console messages with `--enable-logging=stderr --v=0`, and
`--blink-settings=preferredColorScheme=0` forces the dark theme (`1` for
light; `--force-dark-mode` does not affect `prefers-color-scheme`). Wrap it
in `subprocess.run(..., timeout=45)`: the process lingers on macOS. Start
the hub on a spare port (`hub --port 8767`) so a maintainer's running hub
is untouched, and restart it after every edit - the assets are module
constants loaded once.
