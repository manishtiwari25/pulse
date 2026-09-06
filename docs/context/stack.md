# Stack Context

## Framework Stack

PULSE intentionally uses a small, portable stack:

- Markdown for the engineering control plane.
- SVG for the PULSE brand mark.
- Bash plus Python's standard library for optional local usage diagnostics,
  and Bash for the read-only lead-time, board, and evolution reports.
- Python's standard library (`sqlite3`, `http.server`) for the optional
  read-only board dashboard, which projects the board and evolution reports
  into a disposable per-checkout SQLite file in the user's cache and serves
  it on `127.0.0.1` only (ADR-015); the same tool's `hub` mode reads a JSON
  registry in `~/.pulse/` and serves every registered project plus the
  commons, a shared board that is its own Git repository (ADR-016).
- The dashboard's page is a Preact + Tailwind application in
  `docs/scripts/dashboard/` (Vite 7, TypeScript strict, Radix primitives,
  ESLint), built once into three fixed files that are committed and served
  by the same standard-library server (ADR-023). Node is a build-time tool
  for whoever changes the page; a human running the hub needs Python alone.
- Git and GitHub (private repository) for distribution and template adoption.

## Agent Execution Isolation

- GitHub Copilot CLI native local sandbox, with its interactive cloud sandbox
  available when stronger remote isolation is required.
- Claude Code native Bash sandbox.
- Docker Sandboxes or another documented external OS/container sandbox for
  OpenCode, whose built-in permissions are not an isolation boundary.
- Harness permission rules remain defense in depth inside the sandbox.

## Optional Code Context Stack

- Go's standard library for file discovery, chunking, persisted BM25 ranking,
  freshness checks, and the lightweight relationship graph.
- Cross-platform, checksummed release binaries for toolchain-free execution.
- Generated indexes live in the user's operating-system cache.

## Constraints

- No package manager or build dependency is required for the PULSE framework.
- The code-context helper adds no third-party package or model dependency.
- The board dashboard is optional, read-only, and local; its server is
  standard-library only, its page is built ahead of time and committed, and
  `node_modules/` never enters Git; the store stays canonical and the
  SQLite projection is never tracked.
- Running from source requires Go; a published release binary does not.
- No static site: the repository is private and documentation is read in place.
- Agent-controlled execution prefers a sandbox but may continue under normal
  harness permissions when the boundary is reported as unsandboxed.
- Prompts and workflows remain model-agnostic.
- Adopting repositories keep their own language, framework, build, test, and
  deployment choices.

## Future Decisions

Add a dependency or application runtime only when a real PULSE capability
cannot be delivered safely with the current static, portable approach.
