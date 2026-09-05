Playwright browsers and running the headless smoke tests

CI: The project's CI provides a headless Chromium and exposes its install location via
PLAYWRIGHT_BROWSERS_PATH. Tests that call Playwright will use that environment variable in CI so the
smoke test runs headlessly without requiring local browser installs.

Locally: to run the headless smoke test on a developer machine, either:

1) Install Playwright's browsers locally (preferred):
   npx playwright install

   Then run the project's tests:
   python3 -m unittest discover -s docs/scripts/tests

2) Or point PLAYWRIGHT_BROWSERS_PATH at an existing browser install (advanced):
   export PLAYWRIGHT_BROWSERS_PATH=/path/to/browsers
   python3 -m unittest discover -s docs/scripts/tests

Notes:
- If tests still fail because browsers are missing, use option (1).
- CI already sets PLAYWRIGHT_BROWSERS_PATH so no extra steps are required in CI.
