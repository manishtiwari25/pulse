SUMMARY: Added Makefile target `setup-browsers` (runs `PLAYWRIGHT_BROWSERS_PATH=./.playwright-browsers npx playwright install`) and docs/guides/README-setup-browsers.md; verification: unit tests passed (138 tests OK). Measured: local-playwright-setup indicator 0 → 1 (Makefile + docs present).

FINDING: docs/scripts tests guard on PLAYWRIGHT_BROWSERS_PATH and skip when browsers missing; a local helper reduces friction for developers reproducing CI smoke tests.

SUGGESTION: docs/guides: link README-setup-browsers.md from docs/README.md or root README to surface the helper to contributors.
