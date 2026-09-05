# Makefile helpers for local development

.PHONY: setup-browsers

# setup-browsers: install Playwright browsers into a local cache directory
# Mirrors CI by placing downloaded browser binaries under ./.playwright-browsers
# Usage: make setup-browsers
setup-browsers:
	@mkdir -p .playwright-browsers
	@echo "Running: PLAYWRIGHT_BROWSERS_PATH=$(pwd)/.playwright-browsers npx playwright install"
	@PLAYWRIGHT_BROWSERS_PATH=$(pwd)/.playwright-browsers npx playwright install || true
	@echo
	@echo "Local browsers cached under: $(pwd)/.playwright-browsers"
	@echo "To mirror CI for smoke tests, export the following in your shell:"
	@echo
	@echo "  export PLAYWRIGHT_BROWSERS_PATH=$(pwd)/.playwright-browsers"
	@echo
	@echo "Or add it to your shell rc (e.g. ~/.bashrc or ~/.zshrc)."
