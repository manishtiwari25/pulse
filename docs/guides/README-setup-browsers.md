Playwright browsers setup for local smoke tests

CI sets PLAYWRIGHT_BROWSERS_PATH to a cache directory and pre-installs browsers for smoke tests. Local developers can mirror that behavior by running the Makefile helper below which provisions browsers into a repository-local cache and prints the environment variable to set.

Usage:

  make setup-browsers

This will:

- Run `npx playwright install` with PLAYWRIGHT_BROWSERS_PATH set to `./.playwright-browsers`.
- Print instructions for exporting PLAYWRIGHT_BROWSERS_PATH so local smoke tests see the same binaries as CI.

Notes:

- This does not modify CI or any remote configuration.
- npm and npx must be available locally. Do not run this in CI; CI already provisions browsers as part of the job.
