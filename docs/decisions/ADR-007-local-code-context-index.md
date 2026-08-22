---
id: ADR-007
title: Use a Local Retrieval Index and Relationship Graph for Code Context
status: Accepted
date: 2026-08-20
areas: [agents, context, search, code-intelligence]
tags: [adr, go, bm25, graph, retrieval]
---

# ADR-007: Use a Local Retrieval Index and Relationship Graph for Code Context

## Context

Agents often spend time opening many files before they find the small part of
a repository that matters. A reusable context index can do that broad reading
once, then return ranked chunks for later tasks.

The index cannot safely replace source reads. It may be stale, chunk
boundaries may hide nearby behavior, lexical ranking is approximate, and a
simple relationship graph cannot fully understand every programming language.

PULSE must also stay usable without a new package install, hosted service,
hidden repository database, or remote code upload.

## Options Considered

### Option 1: Keep Broad Source Reads as the Only Discovery Path

- **Pros:** no new dependency or generated state.
- **Cons:** every agent repeats expensive exploration and gets no reusable
  semantic or relationship map.

### Option 2: Use Python, SQLite FTS5, and Optional Zvec

- **Pros:** SQLite provides strong built-in text search, while Zvec can add
  local semantic retrieval.
- **Cons:** the helper depends on a suitable Python and SQLite build; semantic
  mode adds native wheels, a model download, and a second dependency path.

### Option 3: Ship a Rust Indexer

- **Pros:** a prebuilt executable can start quickly and avoid Python packages.
- **Cons:** PULSE would need trusted binaries for every operating system and
  CPU, or users would still need a Rust compiler. Semantic embeddings would
  still need an extra model or service.

### Option 4: Use a Standard-Library Go Indexer and Relationship Graph

- **Pros:** one codebase compiles into small cross-platform executables, uses
  no third-party runtime packages, and can implement deterministic BM25
  retrieval plus the graph locally.
- **Cons:** source users still need Go, release binaries and checksums must be
  maintained, retrieval is lexical rather than semantic, and graph coverage
  remains best effort.

## Decision

Choose **Option 4**.

- Add the portable `pulse-code-context` skill.
- Implement the helper in Go using only the standard library.
- Persist a local BM25-style keyword index with path and symbol boosts.
- Build a lightweight file graph from supported imports, includes, sourced
  scripts, and Markdown links.
- Store generated collections and graph data in the operating system's user
  cache, never in a repository-local hidden control folder.
- Skip ignored, binary, oversized, generated, and likely-secret files.
- PULSE bootstrap includes the skill source but does not build an index
  automatically.
- Build tagged release artifacts for supported operating systems and CPU
  architectures, and publish checksums with them. Do not commit binaries.
- Prefer code intelligence and language servers when available. Use the
  context index to narrow discovery before broad reads.
- Require agents to open the exact source ranges before editing and to verify
  changes with the repository's real checks.
- Do not use hosted embedding APIs unless a repository makes a separate,
  explicit privacy and architecture decision.

## Consequences

### Positive

- Repeated agent tasks can reuse a local repository map.
- BM25 ranking, path terms, and symbol terms provide useful local discovery.
- Related-file output helps an agent inspect likely impact boundaries.
- Source code stays on the local machine during indexing and search.
- The helper has no third-party runtime or package dependency.
- A release binary can run without Python, Go, Rust, or a package manager.

### Negative

- Keyword retrieval does not understand semantic similarity.
- Running from source requires Go; toolchain-free use depends on a published
  binary for the user's platform.
- Release artifacts and checksums add a small maintenance surface.
- Index freshness must be checked after source changes.
- The lightweight graph does not replace a compiler, language server, SCIP
  index, or full call graph.
- Agents still need targeted source reads, so the feature reduces broad
  reading rather than eliminating reading.

## Follow-Up

- [x] Add the `pulse-code-context` skill and helper.
- [x] Replace the initial Python implementation with the Go BM25 indexer.
- [x] Add file relationship graph output.
- [x] Add freshness and external-cache checks.
- [x] Document a cross-platform release process with checksums.
- [ ] Keep language-specific graph extraction small and evidence-driven.
- [ ] Consider optional SCIP import if adopting repositories need deeper
  cross-language symbol relationships.
