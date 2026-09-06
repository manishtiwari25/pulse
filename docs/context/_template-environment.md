---
recipe: 1
# Exact pins the runner installs with mise before any command runs (`name@version`, at most 12).
# Leave the list empty when the project needs nothing beyond what the machine already has.
tools: []
# Shell lines the runner runs in a fresh clone, in order, before the check and the worker.
# Absent (as here), the lockfile heuristic runs (uv sync, npm ci, cargo fetch, and the like,
# from the lockfile at the root; WU-021). Present, the list replaces it; `setup: []` runs nothing.
# setup:
#   - make bootstrap
# Plain variables every setup line, check, and worker sees. Names only in briefs and posts.
# A name that looks like a secret (TOKEN, SECRET, PASSWORD, _KEY, CREDENTIAL, PRIVATE) or a
# value shaped like one makes the recipe invalid: secrets never live in this file.
env: {}
# Databases, brokers, or other running services the project needs. Anything listed here makes
# every machine unfit for the project until an owner removes it: the runner starts no services.
services: []
# Advice for the owner and for agents; the project's verify command in the store stays the check.
suggested_check: ""
---

# Environment

What a fresh machine needs before this project's checks can run, in one file the
runner reads at the base commit of every job (ADR-026). Fill the frontmatter from
observation - run the project's own check on a clean clone and record what was
missing - and commit this file alone: a branch that changes it with anything
else is refused, and a branch that changes it by itself waits for a human
landing (`pulse land --project <slug> --branch <branch>`).

## What the runner does with it

1. `tools` - installs each pin with `mise install` on the machine that runs the
   job, then runs setup, check, and worker through `mise exec <pins> --`. A
   machine without `mise` is unfit for the project until it is installed; a pin
   that will not install marks the machine unfit for that tool.
2. `setup` - runs the lines in order in the clone, with `env` exported; a
   non-zero exit ends the job before a worker starts.
3. `env` - exported for setup, check, and worker; secret-shaped names or values
   make the whole recipe invalid.
4. `services` - a non-empty list keeps the project off every machine, with the
   reason `services`, until an owner provides them another way and clears it.
5. `suggested_check` - shown to owners and agents; the verify command the human
   set in the store remains the check that decides.

## System packages

Packages that need root (compilers, headers, `make`) stay the human's apt line;
`pulse env check --project <slug>` lists what each machine still lacks.
