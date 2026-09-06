# Headless browser for the dashboard smoke test

`docs/scripts/tests/test_pulse_dashboard.py::test_headless_browser_smoke`
renders every dashboard view in a headless Chromium and fails on console
errors or CSP violations. It is optional: when no browser is found the test
**skips**, and the rest of the suite is unaffected. There is no CI for this
repository; the check that judges a landing is the local
`python3 -m unittest discover -s docs/scripts/tests`.

The test looks, in order, for `chromium`, `google-chrome`,
`chrome-headless-shell`, `chrome`, or `chromium-browser` on `PATH`, then for
an executable whose name contains `chrome` or `chromium` anywhere under
`PLAYWRIGHT_BROWSERS_PATH`.

To run it locally, either install a Chromium/Chrome that is on your `PATH`,
or download Playwright's build into a directory of your choice and point the
variable at it:

```bash
export PLAYWRIGHT_BROWSERS_PATH="$HOME/.cache/pulse-browsers"
npx playwright install chromium
python3 -m unittest docs.scripts.tests.test_pulse_dashboard -k headless
```

Nothing installs a browser by itself, and nothing in the repository depends
on one being present.
