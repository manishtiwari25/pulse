---
id: PLAN-005
title: Add a Local Code Context Index
status: Done
date: 2026-08-20
tags: [plan, agents, context, search, go, graph]
---

# Plan: Add a Local Code Context Index

## Goal

Give coding agents a small, local map of a repository so they can find the
right files and relationships before opening broad parts of the source tree.
The map must speed up discovery without pretending that cached context is the
source of truth.

## Context

- The first implementation used Python, SQLite FTS5, and optional Zvec.
- The preferred default is now a dependency-free Go implementation that can
  compile into one portable executable.
- Go's standard library does not include SQLite or a vector embedding model,
  so the built-in index uses its own persisted BM25 data and relationship graph.
- Source users need Go to build or run the helper. Truly toolchain-free use
  requires trusted, checksummed release binaries for each supported OS and CPU.
- Generated indexes must live in the user's cache, not in a hidden repository
  folder or committed control-plane path.
- Search results may be stale or incomplete. Agents must still read the exact
  source ranges before editing and use the repository's real checks afterward.

## Steps

- [x] Add a portable `pulse-code-context` skill with an index/search helper.
- [x] Replace the Python helper with a standard-library Go implementation.
- [x] Persist a ranked BM25 keyword index from Git-visible text files.
- [x] Add a lightweight file relationship graph for imports and documentation links.
- [x] Route agents to retrieval-first discovery while keeping source verification mandatory.
- [x] Update framework, bootstrap, skill catalog, and public documentation for Go.
- [x] Document a cross-platform release process for checksummed binaries.
- [x] Test indexing, retrieval, graph traversal, freshness, and cache safety.

## Rollback Plan

- **Baseline:** clean `main` worktree at
  `e474ee20cf118331d78d507e18cf6a8a5bb3a74f`; no code-context skill, helper,
  cache, release workflow, or retrieval guidance exists.
- **Trigger:** helper tests or release builds fail, the generated index cannot
  be queried, generated state appears inside the repository, skill discovery
  breaks, or guidance encourages agents to trust cached context over source.
- **Reversal:** apply a precise inverse patch that removes only the new
  `pulse-code-context` files and restores the specific documentation and
  instruction lines changed by this plan.
- **State safety:** do not add third-party runtime dependencies; tests use
  temporary directories; generated indexes use a dedicated external
  user-cache path; no repository code is sent to a hosted service.
- **Recovery verification:** run the pre-existing skill metadata checks,
  targeted helper tests, `git diff --check`, and confirm the worktree contains
  no generated context database or unrelated reversal.

### Go Pivot Rollback

- **Baseline:** the complete uncommitted Python/SQLite/Zvec implementation and
  documentation were captured before this pivot in the session's
  `files/pre-go-pivot/` rollback bundle.
- **Trigger:** the Go helper cannot preserve the required search, graph,
  freshness, exclusion, cache-safety, or portable skill behavior.
- **Reversal:** restore only the captured code-context files and tracked patch,
  then remove only Go files introduced by this pivot.
- **State safety:** use a new index schema and staged cache replacement so an
  incompatible Go build cannot corrupt a previously generated index. Do not
  commit generated indexes or platform binaries.
- **Recovery verification:** rerun the ten Python helper tests, the original
  SQLite smoke test, skill discovery, link checks, and `git diff --check`.

## Acceptance Criteria

- [x] An agent can build a local keyword index with Go and no third-party packages.
- [x] A release process can produce checksummed binaries for supported platforms.
- [x] A query returns ranked chunks with file paths, line ranges, and related files.
- [x] Ignored, binary, oversized, and likely-secret files are not indexed.
- [x] The generated index stays outside the repository by default.
- [x] The full canonical PULSE skill pack includes the new context skill.
- [x] Agent instructions require exact source verification before changes.
- [x] Verification is complete and recorded.

## Verification

```bash
go test ./docs/skills/pulse-code-context/...
go vet ./docs/skills/pulse-code-context/...
go run docs/skills/pulse-code-context/code_context.go --help
git diff --check
```

Create a temporary repository and cache, build the Go index, query it, inspect
graph neighbors, verify stale detection, and compile every release target.

Completed with eleven Go tests, race detection, 56.9% statement coverage,
`go vet`, no external Go modules, six checksummed cross-platform builds, a
standalone-binary smoke test, live BM25 and graph queries, stale-index
detection, repository-local cache rejection, nine skills discovered by the
universal installer, valid changed HTML and links, and a clean
`git diff --check`.
