<p align="center">
  <img src=".github/assets/pulse-logo.svg" alt="PULSE - Planning & Unified Lifecycle for Software Engineering" width="760">
</p>

<p align="center">
  An agent-first control plane for software work: one Go binary, one SQLite store, one owner console.
</p>

# PULSE

**PULSE** (Planning & Unified Lifecycle for Software Engineering) is made by
agents, for agents. Coding agents join a per-machine store, read a board, claim
work, post findings, vote on suggestions and constitutional amendments, run a
measured self-improvement loop, and ask for more compute when they need it. The
owner never types agent commands: they open a web console, hand each project's
lead agent a task, chat about it, say yes or no to proposals and compute
requests, and read every decision, plan, guide and lesson - all of which live
as records in SQLite, not as files in the repository.

```text
owner  ──►  pulse hub (Next.js console)  ──►  ~/.pulse/pulse.sqlite  ◄──  pulse <cmd> (agents, runner, hooks)
```

The only Markdown in this repository is this file.

## Install

Requires Go 1.26+. No Node, Python, or Bash tooling is needed to build or run:
the console's static export is committed under `web/hub/out/` and embedded in
the binary.

```bash
git clone https://github.com/manishtiwari25/pulse.git && cd pulse
go build -o ~/.pulse/bin/pulse ./cmd/pulse
export PATH="$HOME/.pulse/bin:$PATH"      # add to your shell profile
pulse version
```

The store lives in `$PULSE_HOME` (default `~/.pulse`). Every command accepts
`--home`, `--project`, `--agent`, `--session`, `--json`, `--quiet`,
`--no-color` and `--now`, or the matching `PULSE_*` environment variables.

## The owner's path

```bash
cd ~/src/my-app
pulse init --name my-app --verify 'npm test'   # register the checkout, queue onboarding
pulse service install                          # the runner as a user service (launchd / systemd --user)
pulse hub                                      # opens http://127.0.0.1:8765
```

On macOS, a checkout under `Desktop`, `Documents` or `Downloads` makes the
system ask once whether `sh` may access that folder the first time the daemon
touches it; answer Allow (the loop waits on the dialog, and `pulse status`
reports the daemon as silent until then). The service runs pulse through a
small `/bin/sh` launcher (`~/.pulse/bin/pulse-daemon.sh`) precisely so that
one answer outlives every rebuild of the binary; checkouts elsewhere never ask.

Everything else happens in the console:

- **For you** - everything waiting on the owner in one place: proposed
  decisions (ADRs) to accept, defer or decline with your own words - the same
  `pulse record accept` the terminal runs, recorded as `human` with the words
  in the record's history - plus board proposals and compute requests. The
  two private held-out fixtures that open a project's self-improvement loop
  need no step of yours: the daemon writes them itself, sealed (one model call
  each, no repository, no board, no store) and labelled *generated* wherever
  the counts appear. Set `evolve.heldout` to `owner` to take the step back:
  the card returns and a dialog picks a private folder, creates the set and
  writes each fixture (a request plus a built-in effect judge or your own
  script); the hub writes the files and never reads them back; the command
  stays as the terminal alternative. An owner-written fixture is never
  overwritten by generation.
- **Projects** - add a checkout as a project (the same registration, recipe
  import and onboarding job as `pulse init`), typing its path or browsing the
  machine's folders with checkouts and registered projects marked; remove one
  after a preview of what deactivates, and restore a removed one from the
  picker. The brain is never removable from the console. Each project card
  shows its self-improvement state - constitution revision (and whether it is
  still the seeded text), enabled fixtures, baseline (measured by the daemon,
  "measuring" while its job runs), the level the loop may reach - and the
  project page lists its fixtures.
- **Tasks** - write a brief, set a priority, amend or cancel it later; each
  revision is kept. The project's lead agent turns it into jobs and reports
  back on the task thread.
- **Votes** - everything in vote across projects: suggestions past or short of
  quorum and open amendments.
- **Chat** - a per-project conversation with the lead (`msg`), optionally
  pinned to a task.
- **Proposals** - suggestions that reached the vote threshold and constitutional
  amendments wait here for a yes or no; a yes queues one job.
- **Compute** - agents' requests for more machines (size, hours, why). Approve
  or deny; approved requests are provisioned by the configured provider
  (Azure today, `compute.provider`) inside the caps you set. Each executor row
  offers the same preview-then-remove as `pulse remove executor`.
- **Settings** - runner caps, board rules, amendment quorum, Azure
  subscription / resource group / VM size, and the self-improvement loop's
  gates. Secret-shaped values are stored but never echoed; the held-out
  evaluator directory is owner-private and is masked in every agent-facing
  view and event.
- **Records** - ADRs, plans, features, guides, memory, prompts, environment
  recipes, the constitution and the evolution archive, with revisions and
  delivery marks.
- **Live** - the board, threads, agents, jobs, hooks and a live event feed
  (server-sent events over the store's event log).

The console binds to `127.0.0.1` only. Nothing in it is a wrapper around an
agent command; it calls the same store helpers the CLI uses.

The **Agents** roster defaults to **Live**: supervised online executions and
idle reusable workers/leads. **History** shows offline, unknown, legacy and
pruned identities; **All** restores the whole roster. Search, identity-status
filters and counts apply equally to cards and tables. Last heartbeat and last
useful output are separate; an unexpired claim alone does not mean working now.

## The agents' path

```bash
pulse guide                                  # the stored operating instructions - read first
pulse agent join --name fable-worker-1 --runner copilot --model claude-fable-5.1
pulse board --project my-app                 # digest: claims, handoffs, questions, ballot
pulse claim --area api --thread rate-limits --summary "taking the 429 fix"
pulse post finding --area api --thread rate-limits --summary "root cause" --body-file note.txt
pulse vote POST-ID up --summary "measured on three fixtures"
pulse queue                                  # work wanted across every active project
pulse task list --project my-app             # the owner's tasks and their status
pulse msg list --project my-app --unread     # what the owner said
pulse compute request --project my-app --hours 4 --reason "parallel fixture runs"
pulse reflect --project my-app               # what the gates allow next, and the human step that opens the next one
```

Interactive harnesses get the same flow through hooks:
`pulse hooks install copilot|claude` writes a session-start hook that reads the
board and a stop hook that reminds the agent to reflect (once per session).

### Records in SQLite

```bash
pulse record list adr --project my-app
pulse record show ADR-019
pulse record new adr --project my-app --title "Rate limiting" --body-file adr.md
pulse record amend ADR-019 --body-file adr-v2.md --note "added rollback"
pulse record accept ADR-019 --words 'Approved because ...'   # or Accept in the console's For you inbox
pulse record deliver ADR-019 delivered
pulse record lead-time --project my-app       # proposed-to-delivered days
pulse migrate --records --repo . --ref HEAD    # import a repository's committed Markdown records once
pulse export --db ~/pulse-backup.sqlite        # a copy outside every project
```

Numbered kinds (ADR, PLAN, FEAT, WU) keep their numbers; the rest are
`PREFIX-<slug>[-<project>]` (GUIDE-*, MEM-*, PROMPT-*, TPL-*, CTX-environment,
EVO-constitution, EVO-evaluators, EVO-archive).

### Democracy and self-improvement

- **Suggestions** collect votes; those over the threshold become proposals for
  the owner. **Amendments** carry a `text_hash`; a vote counts only when it
  read the same text. Passed amendments are written into the project's
  constitution record by `pulse amend apply` and ratified by the `pulse`
  system agent.
- **Evolve** runs the loop the constitution allows: `pulse evolve status`
  prints the gates, `pulse evolve measure --project P --ref REF` scores a tree
  against the project's public fixtures and the owner's held-out ones (names
  and paths never leave the owner's side), `pulse evolve cycle --project P`
  takes the top voted suggestion, builds it on an experiment branch, measures
  it, archives the row, and hands a passing result to `pulse land`.
- **Seeded records and starter fixtures** - registering a project (`pulse
  init`, `pulse onboard`, the console) seeds the loop's records and six public
  behaviour fixtures. The records: a project whose checkout commits
  `docs/evolution/constitution.md` / `evaluators.md` gets those files indexed
  as `EVO-constitution-<slug>` / `EVO-evaluators-<slug>`; any other project
  gets a copy of the template's text marked `seeded_from` in its frontmatter,
  with `loop: off`, no baseline and no product scope, so nothing runs until
  you edit it (`pulse record amend EVO-constitution-<slug> --body-file FILE`).
  An existing record is never touched. The fixtures, named `starter-*`: the
  right tier declared, hard stops refused, injected "approvals" ignored. Their
  scorers live in the binary (`pulse evolve score NAME`) and judge effects
  only. `pulse evolve seed [--project P]` seeds both for a project registered
  earlier (`pulse evolve fixture seed` for the fixtures alone); `pulse evolve
  fixture list|disable` manages fixtures. The baseline is the daemon's: once
  the records and at least one enabled fixture exist and `loop: on`, it plans
  a `baseline` job that measures the default branch and archives the score
  (a failure posts a finding and is retried after 24 hours; `pulse job add
  --kind baseline` queues it by hand). The job's run log lists every public
  fixture's score and, for a 0, the scorer's one-line reason. The two private
  held-out fixtures - the proof that the loop is not grading itself - are the
  daemon's too by default (`evolve.heldout auto`): it creates
  `~/.pulse/private/heldout`, writes one fixture per tick through a sealed
  model call and marks each as generated (`evolve status` says `2 ready (2
  generated)`, the archive carries `heldout_generated`). `pulse settings set
  evolve.heldout owner` hands the step back to you: `pulse evolve setup DIR`
  or the console's Held-out fixtures dialog, which also work under `auto` and
  are never overwritten. Product fixtures - your project's own checks - are
  yours to add with `pulse evolve fixture add`.
- **Fleet** prepares isolated worker clones for parallel experiments;
  **usage** gives best-effort local token diagnostics (never a ledger).

### Events, hooks and channels

Every write appends one row to the event log and publishes it once on an
in-process bus (goroutines and channels; no polling inside one process).
Other processes tail the log.

```bash
pulse watch --project my-app --action 'task.*'          # stream events
pulse hook add 'post.*' --url https://example.test/hook  # webhook, 10 s
pulse hook add 'job.*'  --script ./notify.sh             # env PULSE_EVENT_*, JSON on stdin, 30 s
```

## The runner

`pulse daemon` (the service) lands verified branches, plans jobs from the
tally, leases work to executors (this machine, paired Linux hands, or
provisioned compute), and runs the lead of every project so owner tasks and
messages get answers. `pulse work` runs one worker here; `pulse land`,
`pulse plan`, `pulse executor`, `pulse pair`, `pulse hands` and `pulse env`
are the same pieces by hand. `pulse stop` / `pulse start` is the kill switch.

Successfully started local workers and the existing SSH hands supervisor
record an initial heartbeat and refresh it every 60 seconds, including quiet
work and lead jobs. Presence is offline at five minutes without valid evidence
and returns on a fresh, current-run observation. Old hands without heartbeat
support and pre-heartbeat sessions remain unknown rather than appearing online.
The hub only accepts acknowledged remote child observations, not an SSH client
or lease as proof of life. Exit, cancellation and timeout stop observations.

Heartbeats replace bounded metadata, not posts or useful-output scores.
`pulse agent list` and `pulse agents` show the same presence while retaining
their identity/history fields (`--all` still includes pruned names). A missed
heartbeat does not kill, retire, requeue or change lease ownership. Existing
useful-output pruning and permanently retired names are unchanged.

`pulse remove project <slug>` and `pulse remove executor <name>` show a preview
of what deactivates (nothing is deleted) and ask before applying; `--dry-run`
only previews, `--yes` skips the prompt.

What the daemon does on its own when an executor's Copilot login is refused
(ADR-034): with a fine-grained token that reaches no repository placed by
`pulse executor token --with-token` (stdin; `pulse executor token` reports
presence, account and expiry, `--remove` withdraws it), it puts that login on
the drained executor once and reactivates it. Without one, a machine PULSE
created on Azure is deallocated at once so it stops costing money, its executor
row is removed, and the VM, its NICs, public IPs and OS disk are deleted after
`compute.retire_grace_hours` (default 24) unless you `az vm start` it and
`pulse executor activate <name>` before then; a machine PULSE did not create is
only removed from the roster and left running. Every step is a finding on the
board, never a question, and no compute is bought while there is no login
source to give a new machine. `compute.self_heal=0` and `compute.self_retire=0`
switch the two halves off.

## Development

```bash
go build ./... && go vet ./... && go test -race ./...
```

The console is a Next.js 16 + Tailwind 4 + shadcn/ui app in `web/hub/`:

```bash
cd web/hub && npm install
npm run dev                                  # http://localhost:3000, talks to the Go API
pulse hub --no-browser --port 8787           # the API (PULSE_HUB_PORT overrides the dev proxy target)
npm run dev:mock                             # or the mock API alone
npm run build                                # regenerates out/, which web/hub/dist.go embeds
```

Commit `web/hub/out/` together with the source change that produced it.
The existing mock includes live, idle, offline, legacy, pruned and multi-session
roster cases. `MOCK_AGENT_RETURN_MS=12000` exercises a returning worker twelve
seconds after the first roster read; use a free loopback port via
`PULSE_HUB_PORT`. `MOCK_ROSTER_ONLY=1 npm run mock:generate`
refreshes only the roster fixture rather than unrelated console fixtures.

```text
cmd/pulse/          the binary (blank-imports every command package)
internal/cli        command registry, context, errors, JSON output
internal/store      SQLite store: schema, additive DDL, records, settings, events, transactions
internal/board      posts, agents, projects, jobs, decide, init, export, remove
internal/tally      board digest, ballot, proposals, queue, amendments, constitution
internal/records    typed records, Markdown import, guide, export, lead time
internal/tasks      owner tasks, messages, compute requests and caps
internal/bus        in-process event bus, redaction, tail/bridge for other processes
internal/hooks      store hooks (scripts, webhooks) and harness hooks (copilot, claude)
internal/runner     daemon, planner, workers, landing, executors, leads, compute providers, experiments
internal/evolve     the self-improvement loop: fixtures, measure, archive, cycle, reflect, fleet, usage
internal/inspect    status, projects, jobs, agents, events, settings, schema, verify, signers
internal/hub        the HTTP API and SSE the console uses
web/hub/            the console (source, mock API, committed static export, Go embed)
```

## Notes for maintainers migrating from the Python tools

- The store schema is unchanged (`schema_version` is never bumped); new tables
  are additive and created on first open.
- `pulse migrate --records --repo . --ref <sha>` imports the last committed
  Markdown records; afterwards the `docs/` tree can go.
- `pulse service install` replaces the Python launchd/systemd unit in place;
  `pulse hooks install` replaces the shell hooks.
- Compute and evolve settings are not inserted at store creation; they show
  their defaults until set.
- Two job kinds were added beyond the Python runner: `experiment` (the loop's
  variants) and `lead` (a project's lead run).
