---
id: FEAT-004
title: Local Code Context Index
status: Done
date: 2026-08-20
priority: High
areas: [agents, context, search]
tags: [feature, go, bm25, graph]
---

# Feature: Local Code Context Index

## Summary

The `pulse-code-context` skill builds a reusable local map of repository text.
Agents can search that map for relevant code chunks and connected files before
opening exact source ranges.

## User Story

As a developer using coding agents, I want agents to search a local code map
before reading broad parts of the repository so that exploration is faster,
more focused, and reusable across tasks.

## Requirements

### Functional

- [x] Discover Git-visible tracked and untracked files while respecting ignore rules.
- [x] Skip binary, oversized, generated, and likely-secret files.
- [x] Split text into line-addressable chunks with symbol metadata.
- [x] Build a standard-library Go BM25 keyword index.
- [x] Compile the same source into cross-platform release binaries.
- [x] Build a lightweight graph for supported imports, includes, sourced
  scripts, and Markdown links.
- [x] Return paths, line ranges, content, symbols, scores, and related files.
- [x] Detect a missing or stale index.
- [x] Keep generated state outside the repository.
- [x] Support human-readable and JSON output.

### Non-Functional

- [x] **Privacy:** local mode sends no repository text to a hosted embedding API.
- [x] **Portability:** the helper uses only Go's standard library and can ship
  as a single executable.
- [x] **Safety:** indexed context is advisory and cannot replace source verification.
- [x] **Clarity:** missing dependencies and stale indexes are reported explicitly.
- [x] **Recoverability:** a failed rebuild leaves the previous generated index in place.

## Scope

### Included

- Local repository text indexing and retrieval.
- Basic symbol extraction.
- File-level relationship graph construction.
- External user-cache storage.
- Agent instructions and bootstrap distribution.

### Excluded

- A complete language-aware call graph.
- Hosted indexing or remote embeddings.
- Automatic dependency installation during PULSE bootstrap.
- Replacing language servers, SCIP, compiler analysis, tests, or source reads.
- Indexing secrets, ignored data, binaries, or generated dependency trees.

## Interface Changes

The skill adds:

```text
pulse-code-context index
pulse-code-context search
pulse-code-context related
pulse-code-context status
```

Source users can run the same commands with
`go run docs/skills/pulse-code-context/code_context.go`.

## Data Changes

Generated indexes, manifests, and graph data live under the user's
operating-system cache or an explicitly supplied external cache directory.
They are disposable outputs and are not part of the PULSE control plane.

## Decisions

- [ADR-007: Use a Local Retrieval Index and Relationship Graph for Code Context](../decisions/ADR-007-local-code-context-index.md)

## Edge Cases

- **No Go toolchain:** use a trusted release binary for the current platform.
- **No matching release binary:** report the unsupported platform explicitly;
  do not download or execute an unverified fallback.
- **Repository changes during build:** discard the incomplete staged index and ask for a retry.
- **Repository changes after build:** return results with a visible stale warning.
- **Unsupported language:** index text normally; graph and symbol coverage may be limited.
- **No Git repository:** use safe filesystem discovery with generated-folder exclusions.
- **Cache path inside repository:** reject it.

## Rollback Strategy

- **Baseline:** PULSE has no code-context skill or generated retrieval state.
- **Trigger:** incorrect retrieval output, source leakage, repository-local
  generated state, broken skill discovery, or failed helper tests.
- **Reversal:** remove the skill and its documentation references with a
  precise inverse patch; remove only its generated external cache when needed.
- **State safety:** no required dependency or product schema is changed.
- **Recovery verification:** confirm the prior skill pack, instructions, and
  documentation remain valid and no generated database is tracked.

## Generated Prompts

- Skill: `docs/skills/pulse-code-context/SKILL.md`

## Acceptance Criteria

- [x] Unit tests cover discovery, secret skipping, chunks, graph links,
  external cache safety, staleness, and manifest ownership.
- [x] A Go BM25 index can be built and queried without third-party packages.
- [x] Graph queries return incoming and outgoing related files.
- [x] Release builds cover supported macOS, Linux, and Windows targets.
- [x] Framework and public documentation describe the capability consistently.

## Verification Plan

```bash
go test ./docs/skills/pulse-code-context/...
go vet ./docs/skills/pulse-code-context/...
go run docs/skills/pulse-code-context/code_context.go --help
git diff --check
```
