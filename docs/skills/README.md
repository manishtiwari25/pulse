# PULSE Skills

Portable Agent Skills for the PULSE lifecycle. These folders are the canonical
skill source and follow the `skills/<name>/SKILL.md` convention.

Mandatory repository and safety policies remain in `AGENTS.md`. Skills add
detailed, on-demand procedures; they do not replace always-on instructions.

## Available Skills

| Skill | Use it for |
| --- | --- |
| `pulse-bootstrap` | Adding PULSE safely to a new or established repository |
| `pulse-plan` | Creating a verifiable work plan with rollback |
| `pulse-decision` | Recording an ADR and decision trail |
| `pulse-feature` | Defining and delivering a feature through the PULSE lifecycle |
| `pulse-rollback` | Executing a scoped, verified rollback |
| `pulse-review` | Reviewing behavior, evidence, decisions, and rollback readiness |
| `pulse-memory` | Recording durable lessons without storing temporary notes |
| `pulse-delegation-advisor` | Estimating tokens and recommending agent, human, or hybrid work |

## Install with GitHub CLI

Install all skills for GitHub Copilot at user scope:

```bash
gh skill install manishtiwari25/pulse --all \
  --agent github-copilot \
  --scope user
```

Install one skill:

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

Use `--scope project` to install into the current repository's agent skill
location. Generated project or user installation folders are outputs; do not
commit them to the PULSE source repository.

## Install from a Local Clone

```bash
gh skill install . --all --from-local \
  --agent github-copilot \
  --scope user
```

Copilot CLI can also use the canonical directory directly:

```bash
copilot skill add docs/skills
copilot skill list
```

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
