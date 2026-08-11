# Product Context

## Product

**PULSE - Planning & Unified Lifecycle for Software Engineering** is a
docs-first framework for keeping software work understandable from the first
idea through delivery and future learning.

## Target Users

- Individual developers who use AI coding agents.
- Software teams that want decisions, plans, and implementation context to stay connected.
- Maintainers who need a safe way to add a durable engineering control plane
  to new or established repositories.

## Problem

Software work is often split across chat history, tickets, code, and memory.
That makes important context easy to lose, causes agents to guess, and makes
future changes repeat old discussions.

## First Valuable Outcome

A user can point an AI coding agent at the public PULSE repository, paste one
bootstrap prompt, and receive a safe, repository-specific `docs/` control
plane without replacing existing code or rules.

## Product Boundaries

- PULSE provides documentation structure, templates, prompts, workflows,
  agent instructions, and token-usage helpers.
- PULSE does not choose the adopting repository's product stack.
- PULSE does not require a hosted service, hidden state directory, or
  model-specific runtime.
- The source framework stays product-code-free.

## Success Signals

- A new contributor can find the repository's context, decisions, plan, and
  verification path without guessing.
- Different coding agents follow the same lifecycle and control plane.
- Important decisions and durable lessons survive beyond one chat session.
- Token usage is recorded from real runner data without billing information.
