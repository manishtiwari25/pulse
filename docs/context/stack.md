# Stack Context

## Framework Stack

PULSE intentionally uses a small, portable stack:

- Markdown for the engineering control plane.
- Plain HTML and CSS for the public GitHub Pages site.
- SVG for the PULSE brand mark.
- Bash plus Python's standard library for local token collectors.
- Git and GitHub for distribution, template adoption, and public documentation.

## Constraints

- No package manager or build dependency is required.
- The public site must work as static files served from `docs/`.
- Prompts and workflows remain model-agnostic.
- Adopting repositories keep their own language, framework, build, test, and
  deployment choices.

## Future Decisions

Add a dependency or application runtime only when a real PULSE capability
cannot be delivered safely with the current static, portable approach.
