#!/usr/bin/env bash
# Small helper for local developers to install Playwright browsers
# Tier: 1 - convenience script for local devs inside existing ADR boundaries.
set -euo pipefail

echo "Checking Playwright browser installation guidance..."
if [ -n "${PLAYWRIGHT_BROWSERS_PATH-}" ]; then
  echo "PLAYWRIGHT_BROWSERS_PATH is set: $PLAYWRIGHT_BROWSERS_PATH"
  echo "CI sets this path when browsers are preinstalled. If tests still fail, run: npx playwright install"
  exit 0
fi

if command -v npx >/dev/null 2>&1; then
  echo "npx is available. To install Playwright browsers for local smoke tests run:"
  echo "  npx playwright install"
  echo
  echo "This script intentionally does not run the installer automatically to avoid modifying developer machines without consent. Run the command above when you want browsers installed locally."
  exit 0
fi

echo "npx not found. Install Node.js and npm (https://nodejs.org/) or set PLAYWRIGHT_BROWSERS_PATH to point at preinstalled browser binaries used by CI."
exit 0
