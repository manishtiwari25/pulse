SUMMARY: Added .github/workflows/ci-playwright.yml to install Playwright browsers in CI (sets PLAYWRIGHT_BROWSERS_PATH) and run the unit test suite; unit tests passed locally (116 tests).

FINDING: docs/board/governance/dashboard-browser-smoke-test recommends a headless Chromium or Playwright cache; CI workflow now provides browsers via Playwright install.

SUGGESTION: evolution/dashboard-browser-smoke-test: add a short README or note in docs/scripts/ explaining that tests will use PLAYWRIGHT_BROWSERS_PATH in CI and how to run the headless smoke locally (export PLAYWRIGHT_BROWSERS_PATH or run `npx playwright install`).
