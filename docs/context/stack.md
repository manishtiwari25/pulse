# Stack Context

## Framework Stack

PULSE intentionally uses a small, portable stack:

- Markdown for the engineering control plane.
- SVG for the PULSE brand mark.
- Bash plus Python's standard library for optional local usage diagnostics,
  and Bash for the read-only lead-time, board, and evolution reports.
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
