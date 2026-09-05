# PULSE Skills

Portable Agent Skills for the PULSE lifecycle. These folders are the canonical
skill source and follow the `skills/<name>/SKILL.md` convention.

Mandatory repository and safety policies remain in `AGENTS.md`. Skills add
detailed, on-demand procedures; they do not replace always-on instructions.

Every PULSE installation includes the complete canonical pack. The commands
below activate those skills in a runner's native project or user location.
The repository is private, so `gh skill install` needs a GitHub CLI login
with access to it.

## Available Skills

| Skill | Use it for |
| --- | --- |
| `pulse-bootstrap` | Adding PULSE safely to a new or established repository |
| `pulse-template-sync` | Checking upstream PULSE drift and applying approved framework updates |
| `pulse-plan` | Declaring the lifecycle tier, then a Tier 2 plan or a Tier 1 work unit with rollback |
| `pulse-decision` | Recording an ADR with a delivery mark and respecting the work-in-progress limit |
| `pulse-feature` | Defining and delivering behavior at the right tier through the PULSE lifecycle |
| `pulse-rollback` | Executing a scoped, verified rollback |
| `pulse-review` | Reviewing behavior, evidence, decisions, tier honesty, and rollback readiness |
| `pulse-memory` | Recording durable lessons without storing temporary notes |
| `pulse-delegation-advisor` | Estimating tokens with optional history and recommending agent, human, or hybrid work |
| `pulse-code-context` | Building a standard-library Go code index and relationship graph |
| `pulse-sandbox` | Recommending isolation and reporting sandboxed or unsandboxed execution |
| `pulse-reflect` | Always-on self-improvement: observe, record, rehearse, evolve - as far as the gates allow, never refusing; fans out to fleets |
| `pulse-evolve` | Running measured, constitution-bounded self-improvement cycles back to back from the board's voted queue |

## Recommended: Activate the Complete Pack

The universal skills installer discovers the complete bundle set:

```bash
npx skills@latest add manishtiwari25/pulse \
  --skill '*' \
  --agent github-copilot \
  --copy \
  --yes
```

Replace `github-copilot` with another supported agent. Use Node.js 22.20 or
newer. To inspect the pack without installing:

```bash
npx skills@latest add manishtiwari25/pulse --list
```

Update universal-installer copies with:

```bash
npx skills update
```

The universal installer may create `skills-lock.json`. In a full PULSE
repository, `docs/skills/` remains canonical; review any lock-driven update
before it replaces project-customized skill content.

## Alternative: GitHub CLI

Install all skills for GitHub Copilot at project scope:

```bash
gh skill install manishtiwari25/pulse --all \
  --agent github-copilot \
  --scope project
```

Use `--scope user` to activate the same complete pack across repositories.
Installing one skill remains available for advanced customization:

```bash
gh skill install manishtiwari25/pulse pulse-rollback \
  --agent github-copilot \
  --scope user
```

Install all skills for another supported agent:

```bash
gh skill install manishtiwari25/pulse --all \
  --agent claude-code \
  --scope user
```

Generated project or user installation folders are outputs; do not commit them
to the PULSE source repository.

## Install from a Local Clone

```bash
gh skill install . --all --from-local \
  --agent github-copilot \
  --scope project
```

Copilot CLI can also use the canonical directory directly:

```bash
copilot skill add docs/skills
copilot skill list
```

A runner sees only the directories it has registered. A skill added upstream
(for example `pulse-reflect` in v5) does not appear in an adopted repository
until template sync brings it into that repository's `docs/skills/`, or until
you bridge it by hand with `copilot skill add <path>/SKILL.md`, which copies
that one skill to `~/.copilot/skills/<name>/`. Remove the bridge copy with
`copilot skill remove <name>` once the sync lands, or the name appears twice.
A running session lists skills from its start; reload after any change.

Inside an interactive Copilot CLI session, use:

```text
/skills reload
/skills list
/skills info pulse-plan
```

Invoke a skill explicitly by name, for example:

```text
Use the /pulse-plan skill to plan this migration.
```

## Template Sync

Check an adopted repository without changing tracked files or sync state:

```text
Use /pulse-template-sync in check mode.
```

Apply reviewed upstream changes and leave them uncommitted:

```text
Use /pulse-template-sync in apply mode.
```

The skill follows [`template-sync.md`](../workflows/template-sync.md), keeps
project-owned files untouched, and requires approval before merging
review-first instruction changes.

## Sandbox Recommendation Preflight

Use `pulse-sandbox` first for any non-conversational task that may run tools,
commands, builds, tests, scripts, MCP servers, LSP servers, or executables.
The first tool-backed response of a session begins with the matching harness
warning; it returns only when the execution boundary changes or the user asks:

```text
⚠️ SANDBOX RECOMMENDED — use <recommended sandbox> when available; state clearly if execution is unsandboxed. Docs: <matching official sandbox documentation URL>
```

The skill selects the official
[Copilot CLI](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-copilot-cli#running-copilot-cli-commands-in-a-sandbox),
[Claude Code](https://code.claude.com/docs/en/sandboxing), or
[OpenCode Docker Sandbox](https://docs.docker.com/ai/sandboxes/agents/opencode/)
page. The warning is not proof of isolation. Copilot CLI and Claude Code offer
native sandboxes; OpenCode can use an external sandbox because permissions
alone are not an OS boundary. Without a sandbox, the skill reports
unsandboxed execution and continues under normal harness permissions. See
[`sandboxed-agent-execution.md`](../workflows/sandboxed-agent-execution.md).

## Self-Improvement: Reflect, Then Evolve

`pulse-reflect` is the always-on outer loop. It runs at the end of any
tool-backed work, after a signal, on demand, and between cycles, and it never
refuses: it climbs as far as the gates allow - **A** observe (list the
session's signals), **B** record (board posts, memory), **C** rehearse
(constraint checks, an experiment card, an optional multi-agent shadow run),
**D** evolve (hand off to `pulse-evolve`) - and names the exact human step
that opens the next gate. Start with the read-only digest:

```bash
docs/scripts/reflect.sh --area <area>
```

If reflection stops at the held-out gate, the owner chooses a private
directory and the setup helper handles the ids and repository edits:

```bash
docs/scripts/evolution-setup.sh prepare --store /private/path
# Fill H01.md and H02.md.
docs/scripts/evolution-setup.sh activate --store /private/path
```

Fleets - many sub-agents in isolated clones outside the repository, one
orchestrator writing - follow [`fleet.md`](../workflows/fleet.md) with
`docs/scripts/fleet.sh prepare|verify|clean`. Workers never touch the real
tree or the board; a fleet counts as one vote.

`pulse-evolve` runs the cycles for the loop decided in
[ADR-012](../decisions/ADR-012-recursive-self-improvement-loop.md). It refuses
to start unless `docs/evolution/constitution.md` has `loop: on`,
`evaluators.md` names a recorded baseline, `docs/evolution/STOP` is absent,
and no cycle cap the constitution writes is reached; PULSE's default is
continuous, so when a cycle ends the next begins. Read the queue and loop
state first:

```bash
pulse queue
docs/scripts/evolution-report.sh
```

The cycle tests one variable on a sandboxed experiment branch, runs
constraints before scores, appends one archive row, posts one board `result`,
and hands a kept variant to a human merge. Board rules are in
[`board.md`](../workflows/board.md); the ladder is in
[`reflect.md`](../workflows/reflect.md); the cycle is in
[`evolve.md`](../workflows/evolve.md).

## Optional Local Code Context

The `pulse-code-context` bundle includes a standard-library Go helper that
stores its generated index outside the repository.

```bash
go run docs/skills/pulse-code-context/code_context.go index \
  --repo .
go run docs/skills/pulse-code-context/code_context.go search \
  "where session tokens are refreshed" \
  --repo . \
  --json
```

The helper uses no third-party packages, model download, or hosted service.
Use a checksummed release binary when one is available and a Go toolchain is
not. The index narrows exploration; exact source reads and repository
verification remain required.

The documented release process builds binaries for macOS, Linux, and Windows
on AMD64 and ARM64. Verify a downloaded file against the release's
`SHA256SUMS` before running it.

## Update or Remove

```bash
gh skill update
copilot skill remove pulse-plan
```

Check the local command help because supported agents and update flags can
change between CLI versions.

## Skill Maintenance Rules

- Keep the `name` equal to the folder name.
- Make `description` state both what the skill does and when to use it.
- Keep mandatory policy in `AGENTS.md`; do not hide a safety rule only in a skill.
- Keep scripts inside the skill directory so they travel with the bundle.
- Do not pre-approve shell tools unless the skill and script have been fully reviewed.
- Link durable behavior to canonical PULSE features, ADRs, and workflows.
