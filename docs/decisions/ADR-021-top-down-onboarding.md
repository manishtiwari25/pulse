---
id: ADR-021
title: Top-Down PULSE - One Command Onboards a Project, One Command Pairs a Machine, the Daemon Stays the One Supervisor
status: Proposed
date: 2026-09-06
delivery: not started
delivered:
areas: [agents, workflows, governance]
tags: [adr, runner, onboarding, pairing, executors, daemon, agent-first, top-down]
---

# ADR-021: Top-Down PULSE - One Command Onboards a Project, One Command Pairs a Machine, the Daemon Stays the One Supervisor

## Context

The maintainer's vision, in his words (2026-09-06): "in the UI I think we
should have top down approach.. we onboard a project.. and when we make loop
on, agent start working... so the pulse will have readily available agents or
it can spawn agents... one super process will keep all in check and manage
the agents... if we want to add more agent we can just pair pulse in other vm
or machine and we can just work like a single brain.. and more and more."

Most of that shape exists after ADR-019 and ADR-020, but it is reached from
the bottom up, one command at a time:

- **The super process exists.** `pulse daemon` leases jobs, plans new ones
  when a project's queue is empty (the top eligible suggestion, then an
  unanswered question, then one `reflect` a day), spawns a fresh-named
  worker per job, prunes idle agents, probes executors, and fills every free
  slot on this machine and on every pair of hands. `pulse service` keeps it
  alive across crashes and logins (launchd, systemd). Agents are stateless
  processes and the store is their memory, so "readily available agents"
  are the daemon's free slots, not a warm pool.
- **"Pair a machine, work as one brain" exists.** ADR-020 keeps one store,
  one planner, and one landing on the hub and lends the runner other
  machines' hands over a forced ssh command. The first pair, `pulse-vm1`,
  was enrolled on 2026-09-06 (PLAN-020 slice 2).
- **Onboarding exists as `pulse init`,** which registers a checkout and
  queues one onboarding job.

Three gaps keep it from feeling top-down:

1. **Onboarding is several human steps.** `pulse init`, then copying the
   evolution templates by hand, then the evaluators' baseline, then
   `loop: on`. The onboarding job runs once; afterwards the queue is empty
   and the first `reflect` waits for the daily planner, so a fresh project
   sits idle for up to a day before the flywheel (reflect -> suggestions ->
   votes -> improve jobs) starts.
2. **Pairing an existing machine is manual.** The Azure script provisions a
   new VM, but a machine that already exists (a home server, a second VM,
   a colleague's box) needs the hub key generated, `pulse-hands` copied, a
   `pulse` user with one restricted `authorized_keys` line, an sshd `Match`
   block, `executor add`, and `probe` - each a place to get it subtly wrong.
3. **The UI is a window, not a control panel,** and it should stay one:
   ADR-015 and ADR-016 chose a read-only dashboard (GET/HEAD only,
   127.0.0.1, no write path) so that a browser page can never spend credits,
   write repository files, or flip a project's loop. A top-down *view* is
   fine; a top-down *button* is a security-posture change.

The constraints that do not move: the loop's gates are human commits
(`loop: on`, the evaluators' baseline, `STOP`); credentials (the copilot
login on any machine) are never placed by an agent; `pulse decide` is the
human's; hard stops apply in every tier; one store per hub, never federated.

## Options Considered

### Option 1 - Do nothing: document the bottom-up path better

- Pros: no new surface; the runbooks already exist.
- Cons: a fresh project idles for up to a day; pairing stays a ten-step
  checklist; the vision stays a checklist too.

### Option 2 - A control UI: the dashboard onboards projects and flips the loop

- Pros: matches the words "in the UI"; one screen to run everything.
- Cons: reverses ADR-015/016's read-only choice; a local web page gains the
  power to write `docs/evolution/constitution.md`, run `git`, and start jobs
  that spend credits - a cross-site request from any tab could do the same;
  needs auth, CSRF, and an audit trail the CLI already has through the store.

### Option 3 - Federate brains: every machine runs a full PULSE and they sync

- Pros: no single hub.
- Cons: exactly what ADR-020 rejected - split-brain leases, store
  replication, conflicting landings, and a much larger attack surface; the
  fleet is three projects and two machines.

### Option 4 - Two composite commands and one planner rule; the UI stays read-only (chosen)

- Pros: `pulse onboard` and `pulse pair` make the top-down path one command
  each while every existing rule keeps its owner; the daemon is unchanged as
  the one supervisor; the dashboard is untouched (a read-only flywheel view
  can follow later); the whole thing is testable through the store and the
  ssh shim.
- Cons: two new commands to keep honest; `pair` touches a remote machine's
  sshd and users, so its scope must be written down precisely.

## Decision

Adopt Option 4.

### 1. `pulse onboard <path> [--verify CMD] [--name N] [--remote URL]`

One command that does what a human does today by hand, in this order, and
prints what remains human:

1. `pulse init` - register the project, queue the onboarding job (unchanged).
2. **Scaffold the loop's files only when `docs/evolution/` is entirely
   absent**: copy `_template-constitution.md` (as `constitution.md`,
   `loop: off`), `_template-evaluators.md`, `_template-archive.md`; a
   `STOP` file is *not* created (absence means "may run"). A `docs/evolution/`
   that exists with some of those names missing is a **refusal before any
   write** (exit 2, the missing names listed): a half-filled loop folder is
   the owner's to finish, not the command's to guess. Existing files are
   never touched. Nothing is committed or staged: the new paths are printed
   with `git status --short docs/evolution` so the owner sees exactly what
   appeared, because these files are project-owned. A dirty working tree
   elsewhere is not a refusal - untracked files cannot be committed by
   accident, and `onboard` adds nothing to the index.
3. **Queue the first `reflect` job now**, priority just below the onboarding
   job, so the flywheel starts within the daemon's next loop instead of a
   day later: the onboarding job posts findings, the reflect job turns them
   into one-variable `suggestion` cards, later workers vote through `VOTE:`
   lines on the ballot, quorum turns the top card into an `improve` job.
   `onboard` inserts this job itself; it does not depend on the planner,
   whose "a job is already waiting" gate therefore never delays it.
4. Print the two human gates in one block: fill the evaluators' baseline
   (run the project's own checks, record a `B-` row) and set `loop: on` -
   both as human commits - and `pulse service install` if the daemon is not
   running.

`init` stays as it is for scripts that want only the registration. For a
project registered with plain `init`, the **planner rule** applies: when
`plan_jobs` runs for a project that has never had a `reflect` job, it plans
one regardless of the daily cadence - the gate that waits for the project's
queue to be empty still applies, so that reflect follows the onboarding job
(minutes) on the next loop instead of a day later.

### 2. `pulse pair <admin>@<host>[:port] --name <executor> [--workdir /srv/pulse] [--workers 1] [--projects '*'] [--install-copilot]`

One command that turns an existing Linux machine into a pair of hands over
the human's own admin ssh session (the human's key or agent; `pair` never
carries a credential of its own):

1. Generate the hub key `~/.ssh/pulse_hands_ed25519` (no passphrase) if it
   is missing; never reuse a personal key.
2. Over ssh as the admin, with `sudo`: create user `pulse` (locked
   password), install `/usr/local/bin/pulse-hands` from this checkout,
   `mkdir /srv/pulse` owned by `pulse`, write **exactly one**
   `authorized_keys` line (`command="PULSE_HANDS_WORKDIR=... pulse-hands",restrict <hub key>`),
   add the sshd `Match User pulse` block (no password, no forwarding, no
   tty), and set `commit.gpgsign false` and `core.askPass ""` for `pulse`.
   Every step is idempotent; an existing `authorized_keys` with any other
   line is a refusal, not a merge. `--install-copilot` adds Node.js 22 and
   `@github/copilot` on apt-based systems only and is otherwise a refusal
   with the manual steps printed.
   **The sshd change can never lock the admin out.** The block is a drop-in
   (`/etc/ssh/sshd_config.d/60-pulse-hands.conf`); after writing it, `pair`
   runs `sshd -t` and, on any complaint, removes the drop-in and stops
   without reloading. Only a clean `sshd -t` is followed by a reload, which
   leaves the admin's open session untouched; `pair` then opens a **second**
   admin connection to prove the admin path still works, and if that fails
   it removes the drop-in and reloads again through the session it still
   holds. Every remote change is made after every check has passed, so a
   refusal leaves the machine exactly as it was found.
3. Add a `Host <executor>` alias to `~/.ssh/config` pointing at the address
   with the hub key and `IdentitiesOnly yes`, and trust the machine's host
   key from the admin session (`ssh-keyscan` through the already-verified
   connection), so no address reaches the store, the dashboard, or a post.
4. `pulse executor add <executor> --ssh pulse@<executor> ...` then
   `pulse executor probe <executor>`. A probe that fails leaves the machine
   `drained` and prints why (usually: the copilot login, which a human does
   on the machine as `pulse`).

The Azure script keeps provisioning new VMs; `pair` is the second half it
already performs through cloud-init, for machines that exist.

### 3. Who may run them

By default both are the human's, exactly as ADR-020 section 6 and
[`executors.md`](../workflows/executors.md) say today. The one exception is
**machine-checkable**: an agent may run `pair` or `onboard` only when the
store holds a human `pulse decide ... yes` on a board proposal whose body
names the target with a `pair: <executor>` or `onboard: <slug>` line,
decided within the last seven days. When `PULSE_AGENT` is set, the command
looks that decision up before touching anything, refuses with exit 2 when it
is missing or expired, and writes the decision's post id into the
`executor.pair` / `project.onboard` event it logs. One decision covers one
name; it cannot be delegated or widened, and it is spent when the executor
is removed or the seven days pass - a fresh proposal is needed after that.
This is the same trust model ADR-019 gave `pulse decide` itself (a rule the
human owns, a row the store keeps, an event the log shows), and it turns the
prose authorization used today into a row. ADR-020 section 6 and the "Never"
paragraph of `executors.md` are amended by this section to say so. In both
commands the probe, not the command, decides whether a machine receives
jobs, and `loop: on`, the baseline, the copilot login, and `pulse decide`
stay human.

### 4. The dashboard stays a window - and is not touched in this cut

ADR-015 and ADR-016 stand: no write path, GET/HEAD only, 127.0.0.1. This
decision changes nothing in the dashboard. A read-only **Flywheel** view
(onboarded at, first reflect, first eligible suggestion, first improve job,
first kept commit, the elapsed time between each) is worth having and is
listed under Follow-Up as its own Tier 1 work unit under ADR-015, after the
commands have run for real - the measurement below needs no view.

### 5. Measurement

The score this decision moves is **time from `pulse onboard` to the first
kept commit** on a fresh project, read from the store by one query the plan
records (`project.add` event to the first `done` run with `commits > 0`), plus the
number of human steps between (target: two - baseline and `loop: on`). The
`pair` half is measured by the test suite's ssh shim (golden remote script,
every refusal) and by `pair --dry-run` against the machine cloud-init
already prepared, which must report every step as present and change
nothing.

### 6. Deliberation

Board thread `runner/top-down-onboarding`, suggestion
`20260906T043643Z-fable-scale-09051203`. A three-lens panel (reliability,
security, value; `panel-*-0906a`) returned three "no" verdicts in round one,
each with a concrete ask: `sshd -t` before any reload and a refusal for a
partial `docs/evolution/` (reliability); a machine-checkable authorization
for an agent-run `pair` (security); the Flywheel view and the fresh-VM proof
cut from the first cut (value). All were folded into sections 1-5 above; one
sub-ask - refusing on a dirty working tree - was rejected with its reason in
section 1. Round two: three "yes", nothing remaining. The agreed proposal is
`20260906T045241Z-fable-scale-09051203`; the human's `pulse decide` on it
is the approval this ADR waits for.

## Consequences

### Positive

- Onboarding is one command and two named human commits; a fresh project's
  flywheel starts within one daemon loop.
- Pairing an existing machine is one command with the hardening checklist
  applied the same way every time; the Azure path and the pair path produce
  identical machines.
- Nothing about ownership changes: humans keep the gates, agents keep the
  work, the daemon keeps the fleet, the dashboard keeps watching. The
  authorization an agent needs to pair a machine becomes a row the store can
  check instead of a sentence in a memory file.
- The rejected options (a writing UI, federated brains) are recorded, so the
  next "make it top-down" request starts here.

### Negative

- `pair` edits sshd and users on another machine; a bug there could lock the
  admin out or leave a permissive line. Mitigation: checks before the first
  change, idempotent steps, refusal on anything unexpected, `sshd -t` before
  any reload, a second admin connection before letting go, a dry run that
  prints every remote command, and the probe as the final judge.
- Two more commands to test and keep in sync across adopters through the
  template sync.

## Follow-Up

- [ ] PLAN-021 approved by the human (`pulse decide` on the board proposal)
      before any code changes.
- [ ] Implement `pulse onboard`, `pulse pair`, and the first-reflect planner
      rule behind tests (ssh shim for `pair`).
- [ ] Update `docs/workflows/pulse.md`, `docs/workflows/executors.md`
      (the "Never" paragraph and ADR-020 section 6's exception),
      `docs/context/pulse-store.md`, `AGENTS.md`, `README.md`, the Copilot
      and Claude instruction files.
- [ ] Later, as a Tier 1 work unit under ADR-015: the read-only Flywheel
      view, once the store holds a few real onboardings to show.
- [ ] Record memory if a reusable lesson appears.
- [ ] Update `delivery:` and `delivered:` when PLAN-021 closes.

## Delivery

The frontmatter `delivery:` mark is the source of truth for whether this
decision has been built:

- `not started` - set when the ADR is created.
- `partially delivered` - the first plan step has landed.
- `delivered` - the plan closed; set `delivered: YYYY-MM-DD`.
