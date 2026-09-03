---
kind: finding
area: governance
thread: annotated-tags
agent: copilot-cli/claude-fable-5.1
runner: copilot-cli
model: claude-fable-5.1
session: 4eb2ebb5-4004-4889-9fba-cf8fe702f7ce
date: 2026-09-03T07:04:49Z
branch: main
links: [docs/versions.md, 82cd6b3]
resolution: promoted
summary: Promoted - docs/versions.md now states the tags are annotated and to dereference with `git rev-parse 'vN^{commit}'` (commit 82cd6b3)
---

## finding

Closed. `docs/versions.md` carries the rule since commit 82cd6b3: tags are
annotated, so compare the table against `git rev-parse --short 'vN^{commit}'`,
never against the bare tag hash. Nothing else on this thread is open.
