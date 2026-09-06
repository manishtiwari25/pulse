---
id: ADR-022
title: Git-Native Agent Identity - Every Agent Commit Names Its Agent, Carries Its Job, and Is Signed by a Key That Lives Only for That Run
status: Accepted
date: 2026-09-06
delivery: delivered
delivered: 2026-09-06
areas: [agents, governance, security]
tags: [adr, agents, runner, git, identity, signing]
---

# ADR-022: Git-Native Agent Identity

## Context

PULSE agents are identities, not models (ADR-019): every agent joins the
store once with a unique name, and the runner already writes that name into
each throwaway clone before a worker commits - `user.name = <agent>`,
`user.email = <agent>@pulse.local`, signing off (`pulse.py`, `SshExecutor`
and the local runner; `pulse-hands.sh` `prepare`). Kept commits land on a
local `pulse/<agent>/<job>` branch and a human merges and pushes them
(ADR-019, ADR-020). So the git log already says *who*: `aurora` authored
this, `cinder` that, and the human's merge commit stays the human's.

What it does not say is anything a reader can check:

- **Nothing binds a commit to its agent.** `user.name` is a config line. A
  worker - or anything else that reaches a clone - can commit as any name.
  The brief tells the worker "never sign as anyone else", which is a rule,
  not a mechanism.
- **A commit does not know its job.** The store has the run, the job, the
  executor, the model, the credits; the commit has an author and a date.
  From a commit in the merged history there is no path back to the run row
  that produced it except by matching timestamps.
- **The identity ends at the machine.** When the human pushes, GitHub shows
  an unlinked author named `aurora` and nothing else.

A maintainer pointed at a popular alternative (one GitHub App per agent,
commits as `agent[bot]` with a noreply address, hourly installation tokens)
and then asked for a solution that is **git, not GitHub**. That is the right
instinct for PULSE, for three reasons that are already decisions:

1. The hands never hold a token the hub did not put there, never push, never
   have a remote (ADR-020). A GitHub App identity is a push credential; giving
   it to agents reverses the design.
2. Agents are many and short-lived - workers spawn with fresh names and are
   pruned after `prune_after_days` (ADR-019). A GitHub App per agent is a
   human ceremony per agent; it does not scale to a machine that spawns them.
3. PULSE runs against any git host, or none. The audit trail must be readable
   with `git log` on the machine, not on one vendor's page.

Git already has everything needed: an author, message trailers, SSH commit
signatures (git >= 2.34; `ssh-keygen -Y`), and `gpg.ssh.allowedSignersFile`
for verification. A prototype on 2026-09-06 (macOS git 2.50; the paired VM
runs git 2.43, which also supports it) showed the whole chain with the worker
doing nothing but `git commit`:

```text
author/committer: aurora <aurora@pulse.local>
trailers:         Pulse-Agent: aurora  Pulse-Job: job-1  Pulse-Executor: pulse-vm1
hub verify:       G aurora@pulse.local      (the run's key)
impersonation:    U                          (another key: untrusted)
unsigned:         N                          (no signature)
```

One trap surfaced too: a machine-wide `gpg.ssh.program` (a password
manager's signer, on the maintainer's laptop) hijacked the signing until the
clone pinned `gpg.ssh.program = ssh-keygen`. The clone must own every
signing knob it depends on.

## Options Considered

### Option 1 - Status quo: name and address, signing off

- Pros: nothing to build; the author is already the agent's name.
- Cons: a name is a claim, not evidence; no path from a commit to its run;
  impersonation costs one config line.

### Option 2 - One GitHub App per agent (the `agent[bot]` pattern)

- Pros: a badge on the host's page; scoped repository permissions; tokens
  rotate hourly; no personal credential on the server.
- Cons: a push credential in agents' hands, against ADR-020; a manual App
  per agent when agents are spawned and pruned by a daemon; tied to one
  host; still no binding of commit to run; the `[bot]` badge is rendered by
  GitHub, not verifiable with git.

### Option 3 - Long-lived SSH signing key per agent

- Pros: one key per identity mirrors "agents, not models"; `git log` verifies.
- Cons: a private key per agent to store, rotate, and revoke on prune; the
  key must travel to every pair of hands the agent may run on; a leaked key
  impersonates the agent until revoked.

### Option 4 - Per-run SSH signing key, provenance trailers, hub verification (chosen)

- Pros: no long-lived secret anywhere - the key is born for one run, signs
  that run's commits, and is destroyed with the clone; the hub verifies every
  commit it lands against exactly that key, so impersonation and unsigned
  commits are refused before they reach a branch; the public key is a column
  on the run row, so any commit's `Pulse-Job` trailer leads to the run, the
  agent, the model, the executor, and the key that can verify it; works on
  every git host and with no host at all.
- Cons: GitHub (and other hosts) will show agents' commits as "Unverified"
  because no account holds these keys - verification is git-local by design;
  ~30 lines across `pulse.py`, `pulse-hands.sh`, and tests; one more verb on
  the hands.

## Decision

Adopt Option 4. Every commit a PULSE worker makes is **authored by its
agent, annotated with its job, and signed by a key that exists only for that
run**; the hub **verifies before it lands**. The human's own commits, merges,
and pushes are untouched.

### 1. What the clone is configured with (both executors)

`prepare` (local runner and `pulse-hands.sh`) sets, at clone level:

```text
user.name             <agent>
user.email            <agent>@pulse.local
gpg.format            ssh
gpg.ssh.program       ssh-keygen          # pin: never a machine-wide signer
user.signingkey       <job dir>/run.key   # 0600, outside the clone
commit.gpgsign        true
tag.gpgsign           false               # workers never tag
```

and installs one hook, `.git/hooks/prepare-commit-msg`, that appends the
provenance trailers with `git interpret-trailers --if-exists doNothing`:

```text
Pulse-Agent: <agent>
Pulse-Job: <job id>
Pulse-Executor: <executor name>
```

Hooks are local to the clone and never committed. Copilot's own
`Co-authored-by` trailer is unaffected.

### 2. The key's life

- The **hub** generates an ed25519 key with no passphrase per run
  (`ssh-keygen -q -t ed25519 -N '' -C '<agent> <run id>'`), stores the
  **public** half on the run row (`runs.signing_pubkey`), and never stores
  the private half in the store.
- **Local runner:** the private key is written to the job directory (0600)
  and removed with it.
- **Remote hands:** a new verb `key JOB` receives the private key on stdin
  and writes `<workdir>/JOB/run.key` (0600, owner `pulse`); `clean JOB`
  removes it with the job directory. The key crosses the wire once, over the
  same forced-command ssh session every other verb uses, and grants nothing:
  it can only sign. `probe` keeps reporting `remotes=0 keys_ok=yes`; a
  leftover `run.key` outside a live job directory is a probe failure.

### 3. What the hub verifies before landing

After fetching the clone's branch, the hub writes a one-line allowed-signers
file for that run only (`<agent>@pulse.local namespaces="git" <pubkey>`)
and runs `git log --format='%H %G? %(trailers...)' BASE..HEAD`. A commit is
landable only if its status is `G`, its author is the leased agent, and its
`Pulse-Agent` and `Pulse-Job` trailers match the run. Anything else -
`U`, `N`, `B`, a foreign author, a missing or wrong trailer - fails the run
with a plain reason ("commit abc123 is not signed by this run's key"), lands
nothing, and posts the usual `result`. This is a constraint, checked before
scores, in the sense of `docs/evolution/evaluators.md`.

### 4. Reading it back

- `pulse records` and the run's row show `signing_pubkey`; `pulse verify
  <ref>` (new, read-only) resolves a commit's `Pulse-Job` trailer to its run
  and verifies the signature against the stored key, so a maintainer can
  audit any commit in the merged history months later.
- `pulse signers` (new, read-only) prints an allowed-signers file for every
  run the store knows, for `git -c gpg.ssh.allowedSignersFile=... log
  --show-signature` on any checkout.
- The dashboard, when it chooses to, reads the same columns; nothing here
  requires it.

### 5. Who may do what

- This is **Tier 2** (identity and security posture): a human approves this
  ADR before any of it is built. No GitHub credential, account, or App is
  involved; the keys are not access credentials.
- The change to `pulse-hands.sh` reaches paired machines only through
  `pulse pair` (which refreshes the hands script; ADR-021 §3 gate), never by
  hand.
- Rollback is a revert of the delivering work unit: clones go back to
  `commit.gpgsign false` and no hook; already-landed signed commits stay
  valid history.

### 6. Measurement

- Constraint: every commit on a `pulse/<agent>/<job>` branch verifies `G`
  against its run's key and carries matching trailers - checked by the runner
  on every run and by a test that lands one signed run, refuses one
  impersonated commit, and refuses one unsigned commit.
- Observation after 20 runs: how many runs failed on the identity check
  (expected 0 when workers behave), and whether the extra `ssh-keygen` per
  run is visible in run seconds (expected < 1 s).

## Consequences

### Positive

- A commit is evidence, not a claim: which agent, which job, which hands,
  verifiable with git alone, on any host or none.
- No long-lived secret: nothing to rotate, revoke on prune, or leak. A
  compromised hands machine can sign nothing after `clean`.
- The store and the history finally point at each other: a trailer leads to
  a run row; a run row verifies a commit.
- Fits the machine that spawns and prunes agents: identity costs one key per
  run, made and destroyed by the runner.

### Negative

- Hosts show agents' commits as "Unverified" (unknown key). That is honest:
  the host cannot verify them, the maintainer can. Anyone who wants a host
  badge for the *human's* merges already has it from the human's own key.
- A worker that points `core.hooksPath` elsewhere (`git -c
  core.hooksPath=/nonexistent commit`) skips the hook; the hub then refuses
  the commit for its missing trailers, so the run fails rather than landing
  unlabelled work. That is the intended outcome, but it is a failed run, not
  a repaired one. (`git commit --no-verify` is not a bypass: it skips only
  `pre-commit` and `commit-msg`, and `prepare-commit-msg` still stamps the
  trailers.)
- One more verb on the hands and one more column in the store; a migration.

## Follow-Up

- [x] Human approval of this ADR (`status: Accepted`, 2026-09-06 by
      `pulse decide`), then a Tier 1 work unit under it, [WU-013](../plans/WU-013-git-native-agent-identity.md): `pulse.py` (key per run, clone config, hook, landing
      check, `runs.signing_pubkey`, `pulse verify`, `pulse signers`),
      `pulse-hands.sh` (`key` verb, `clean`, probe), tests for the signed,
      impersonated, and unsigned cases on both executors.
- [x] Update `docs/context/pulse-store.md` (the new column and commands) and
      the worker brief line about signing (2026-09-06, WU-013).
- [ ] Refresh paired machines with `pulse pair` once delivered (`pulse-vm1`:
      see WU-013's closure line).
- [x] Record the `gpg.ssh.program` pin as a memory pattern
      (`docs/memory/patterns.md`, 2026-09-06).
- [x] Update `delivery:` and `delivered:` when the work unit closes
      (`delivered`, 2026-09-06).

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan or work-unit step has landed.
- `delivered` - the plan or last work unit closed; set `delivered: YYYY-MM-DD`.
- `retired` - the decision is superseded, deprecated, or abandoned.

Built by: [WU-013](../plans/WU-013-git-native-agent-identity.md).
