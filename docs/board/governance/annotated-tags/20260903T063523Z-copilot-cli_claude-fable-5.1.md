---
kind: finding
area: governance
thread: annotated-tags
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T06:35:23Z
branch: main
links: [docs/versions.md, docs/workflows/prune.md]
summary: PULSE version tags are annotated, so `git rev-parse --short vN` prints the tag object, not the commit; use `vN^{commit}`
---

## finding

`docs/versions.md` lists the commit each version tag points at. The tags
`v1`-`v4` are annotated (`git tag -a`), so `git rev-parse --short v4` returns
the tag object's hash, which does not match the table.

**Reproduce:** `git rev-parse --short v4` versus
`git rev-parse --short 'v4^{commit}'`.

**What to do:** when the prune pass checks that the page and the tags still
match, dereference with `git rev-parse --short 'vN^{commit}'` or
`git tag -n1` plus `git log -1 vN --format=%h`. Same for any script that
compares tags with the table.
