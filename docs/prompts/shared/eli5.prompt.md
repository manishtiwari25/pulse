---
id: S-ELI5
title: Shared ELI5 Output Format
status: Active
date: 2026-08-07
target: shared
tags: [output-format]
---

# Shared Prompt - ELI5 Output Format

The **default** output format for every runner (Claude Code, GitHub Copilot CLI, OpenCode, Codex, Cursor, or any other agent reading `AGENTS.md`) and every model running inside them — the format follows the repository, not the model. Explain everything as if the reader is smart but completely new to the topic.

## When It Applies

Applies to **all LLM-produced output**, not just final answers:

- **Every conversation message:** answers, progress/status updates between steps, plans presented for approval, findings, error reports, and questions back to the user — all written in plain words. Even a one-line status note ("Now checking the login code for the bug") must be understandable without jargon.
- **Prose in produced artifacts:** plans, ADRs, feature specs, memory notes, commit messages, PR descriptions, and README text are written in easy-to-understand language. Artifacts keep their required template structure (an ADR still has its sections); the *language inside* the sections is plain. The full takeaway → steps → recap shape is for conversation messages, not forced into doc templates.
- **Code and configs are the exception:** source code, configs, and commands stay real and idiomatic — the plain-word explanation wraps around them.
- **Opt out per request** when the user writes `normal`, `technical`, `no eli5`, or asks for a terse/expert answer. Return to ELI5 on the next request unless the user asks for the opt-out to stay on for the session.

## How to Switch

Say any of these in a request — no command, flag, or setting is needed; the words themselves switch the format:

| You want                              | Say                                                              | Lasts               |
| ------------------------------------- | ---------------------------------------------------------------- | ------------------- |
| A technical answer, just this once    | `normal`, `technical`, or `no eli5` (e.g. `technical: why did the build fail?`) | this request only   |
| Technical answers from now on         | `switch to technical for this session` / `turn off eli5 for this session` | rest of the session |
| Back to ELI5 after switching off      | `eli5` / `switch back to eli5`                                   | per the phrasing    |
| ELI5 for one answer while switched off | `eli5: <question>`                                              | this request only   |

Agents: honor these switches immediately, confirm the mode change in one short line, and never carry a session-wide switch into a new session — every new session starts in ELI5.

To change the **repository default** itself (make technical the default again), edit the "Output Formats" section in `AGENTS.md`, `CLAUDE.md`, and `.github/copilot-instructions.md`, and update this file's "When It Applies" section to match.

## Format Rules

- For tool-backed work, print the mandatory `pulse-sandbox` recommendation
  warning before the ELI5 takeaway. The warning stays exact for the selected
  harness, includes its matching sandbox documentation URL, and is not
  rewritten into simpler language.
- Lead with the one-sentence takeaway in plain words before any detail.
- Use everyday language: no jargon, acronyms, or tool-internal shorthand without an immediate plain-word meaning ("a lockfile — a list that pins the exact versions we installed").
- Prefer one concrete analogy from daily life when it genuinely clarifies; drop it if it strains.
- Keep sentences short and steps numbered. One idea per sentence.
- After the simple explanation, add a short **"In technical terms"** recap (2-4 lines) so the reader can map the simple story back to the real names, files, and commands. The closing message of a piece of work is the exception: see the next section.
- Keep it honest: if something is uncertain, risky, or has trade-offs, say so in simple words instead of hiding it.

## The Closing Message: One Proposal, One Yes or No

The maintainer decides with a yes or a no, not by reading a technical dump and
choosing among options. When a piece of work ends with something the human has
to decide, the closing message follows these rules (decided with the
maintainer on 2026-09-05; procedure in `docs/workflows/board.md`,
"Deliberation"):

- **Agents discuss first.** Settle the decision among agents on the board -
  a `question`, at least two other identities' `opinion` posts, then one
  `answer` with `resolution: agreed` - before the human hears about it.
- **One proposal, not a menu.** Say what the agents agreed on in plain words:
  what changes, what stays, what it costs, what the human must do. Never list
  options and ask the human to pick; if the agents split, give the majority
  position and name the dissent in one sentence.
- **One question.** End with exactly one yes-or-no question per proposal
  (`Say yes and I do it; say no and nothing changes`). Every proposal that
  `pulse proposals` lists as `awaiting` is repeated this way, one question
  each, until the human answers.
- **Short.** About 120 words for the whole closing message; no "In technical
  terms" recap in chat. The technical detail lives in the `pulse` post and the artifact; the message
  ends with `Details:` and that post id.
- **Record the answer.** When the human says yes or no, record it with `pulse decide <post-id>
  yes|no --words '...'`, then act on a yes - within the tier and the hard
  stops, which the human's yes never removes.

## What Does Not Change

- Facts, numbers, file paths, and commands stay exact — simplify the language, never the truth.
- Code blocks stay real, runnable code; the plain-word explanation goes around them, not inside them.
- Repository rules still apply: control-plane routing and verification are
  unchanged and are not rewritten in ELI5 style.

## Example

Request: `why did the build fail?`

Response shape:

> The build failed because the project asked for a tool version the machine does not have — like a recipe calling for an oven setting your oven does not go up to.
>
> 1. The project says it needs Node 22 (in `.nvmrc`).
> 2. This machine has Node 18.
> 3. The build tool checked, saw the mismatch, and stopped on purpose.
>
> To fix it, install the newer version and run the build again.
>
> **In technical terms:** `.nvmrc` pins Node 22; the CI step `node --version` gate exited 1 on Node 18. Run `nvm install 22 && nvm use 22`, then re-run the build command.
