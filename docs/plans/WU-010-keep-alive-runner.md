---
id: WU-010
title: A Keep-Alive Machine - No Daily Cap, a Daemon That Stays Up, and Runs That Feed the Next Runs
status: Done
tier: 1
date: 2026-09-05
delivered: 2026-09-05
controlling_adr: ADR-019
areas: [agents, workflows, governance]
tags: [work-unit, runner, daemon, keep-alive, cap, suggestions, service]
---

# Work Unit: A Keep-Alive Machine

The maintainer said on 2026-09-05, with the runner visibly working after
WU-009: "I want keep alive system.. no need to set job per day" and "keep
improving, keep making next version better". Three things stood between the
runner and that:

1. The daily cap. `jobs_per_day` had no spelling for "none"; at 2 the daemon
   spent its day by 10:00 UTC and slept until midnight.
2. The daemon's life. It was one `nohup` process: a crash, a logout, or a
   reboot ended it; an exception anywhere in a loop ended it; a merged fix to
   `pulse.py` reached it only after a hand restart; and nothing told
   `pulse status` whether it was alive at all.
3. The queue's food. Workers could report `FINDING:` and `QUESTION:` lines
   but had no way to leave an improvement card behind, so the queue depended
   on humans and one `reflect` a day to think of the next thing. And the
   planner built any eligible suggestion, including one the voters had turned
   down.

This work unit changes those three inside the runner ADR-019 already decided.

## Tier Declaration

- **Tier:** 1 - the runner's caps, its daemon, and its result lines are the
  agents' machine ADR-019 §4-§5 describes; a cap's value, a service file in
  the user's own home, and a new result-line kind change no decision.
- **Controlling ADR:** [ADR-019: Agent-First PULSE Store and Runner](../decisions/ADR-019-agent-first-pulse-store-and-runner.md)
- **Why this stays inside the ADR:** §5 says the caps are settings a human
  changes; `0` is one more value of one of them, set here at the human's
  explicit words. The daemon's job (plan, lease, run, prune, until stopped)
  and the per-run caps (`minutes_per_job`, `max_ai_credits`), the kill switch,
  the denied tools, and the human merge are unchanged. A `suggestion` post
  from a worker is the same kind of post any agent already writes, with the
  worker's provenance. `pulse service install` writes one file under the
  user's home and loads it - the same machine, the same login, the same
  store - and is run only at the human's request.
- **Hard stops touched:** none. Spend stays inside the maintainer's own login
  and per-run caps; no credential, remote, push, merge, or infrastructure
  changes. The service registration is a machine setting the human asked for
  in words; it is recorded as a `result` marked `proceeded`.

## Outcome

The machine stays up and stays fed. Set `jobs_per_day 0` and the daemon runs
one job after another for as long as jobs, eligible suggestions, open
questions, or the day's reflect exist. Install it once as a user service and
it comes back after a crash, a logout, or a reboot; a bad loop is printed and
retried, not fatal; a merged change to `pulse.py` restarts it by itself;
`pulse status` says `daemon alive pid N, last loop 12s ago`. Every worker can
leave `SUGGESTION:` lines that become cards on the next workers' ballot, and
the planner builds only the cards voted up on balance.

## Behavior

### User Story

As the maintainer, I want the agents' machine to keep working and keep
improving the repository without a daily quota or a hand restart, so that the
next version is better every morning and my part is reviewing branches and
answering proposals.

### Requirements

- [x] `jobs_per_day` `0`, `none`, `off`, `unlimited`, or empty means no daily
      cap; any positive integer is the cap as before; `pulse status` says which.
- [x] The daemon writes a heartbeat every loop (`meta` key `daemon`) and
      `pulse status` reports `alive`, `silent`, or `exited` from it.
- [x] An exception inside planning or a run is printed as `daemon error:` and
      the loop continues after one interval; the connection is rolled back.
- [x] When `pulse.py` changes on disk the daemon returns `restart` and the CLI
      re-executes itself with the same arguments (`--no-restart` disables).
- [x] `pulse service install|uninstall|status|show [--interval N]` writes and
      loads a launchd agent on macOS (`RunAtLoad`, `KeepAlive`) or a
      `systemd --user` unit on Linux (`Restart=always`), baking in the current
      `PATH`, Python, absolute `pulse.py`, `PULSE_HOME`, and the log path.
- [x] Worker `SUGGESTION: [area/thread:] card` lines become `suggestion`
      posts with the worker as author; default `evolution/<job-id>` (the job's
      own `result` closes `jobs/<job-id>`, which would bury the card); the
      brief explains the line.
- [x] `plan_jobs` skips eligible suggestions whose score is not above zero.
- [x] The store, the immutability triggers, votes, quorum, and the per-run
      caps are unchanged.

### Interface Changes

- `pulse settings set jobs_per_day 0` (a human's command; an agent runs it
  only quoting the human's words).
- `pulse status`: `runs today N (no daily cap; jobs_per_day=0)` and a
  `daemon` line.
- `pulse daemon --no-restart`.
- `pulse service install|uninstall|status|show [--interval N]`.
- Brief rule 6 gains the `SUGGESTION:` line; `PULSE_RESULT.md` accepts it.

### Data Changes

None to the schema. One new `meta` row (`daemon`), rewritten every loop, and
one new value of an existing setting. Both are allowed by ADR-019 (`meta` is
the store's own facts; settings are the caps).

### Edge Cases

- No cap and no work: the daemon is idle and sleeps one interval; no spend.
- No cap and a runner error (bad flag, missing login): the error run does
  not spend anything, the job requeues up to three attempts, the daemon
  waits one interval - as WU-009 decided. Spend is bounded by time: at most
  about six ten-minute runs an hour, each up to `max_ai_credits`.
- The service manager's environment lacks Homebrew's `PATH`: the plist and
  unit carry the installing shell's `PATH`, so `copilot` and `git` resolve.
- A worker writes `SUGGESTION:` with a malformed prefix (`Evolution/Speed:`,
  a URL): the whole line is the card, filed under the default thread.
- The worker's model is too expensive for `max_ai_credits`: the CLI ends the
  turn with "No response was returned" (`session_limits_exhausted`) and the
  runner records an `error` run and requeues; the fix is a cheaper
  `worker_model` or a larger cap, both the human's settings.
- The daemon's own code is broken by an edit: launchd or systemd restarts it
  every 30 seconds and the log shows the traceback; `pulse service
  uninstall` or reverting the file stops that.
- `pulse stop` under the service: the process stays alive, prints `stopped`,
  exits, and the service manager restarts it; each new process checks the
  switch first and exits again until `pulse start`. Uninstall the service to
  stop the restarts.

## Steps

- [x] `daily_cap` and the cap check in `work_once`; `status_text` wording.
- [x] `parse_suggestion_line`, `SUGGESTION` in `parse_result_file`, brief
      rule 6, suggestion posts in `run_job`.
- [x] Positive-score guard in `plan_jobs`.
- [x] `daemon_heartbeat`, `daemon_status`, `source_stamp`; try/except and
      restart in `daemon`; `execv` in `main`.
- [x] `service_paths`, `service_file_text`, `service_commands`,
      `install_service`, `uninstall_service`, the `service` subcommand.
- [x] Six tests in `docs/scripts/tests/test_pulse.py`.
- [x] `docs/workflows/pulse.md`, `docs/context/pulse-store.md`, this unit,
      `docs/plans/README.md`.
- [x] Apply on the maintainer's machine at his words: `jobs_per_day 0`, stop
      the old `nohup` daemon, `service install --interval 300`, watch the
      first run.
- [x] What the first runs taught, fixed the same hour: every real worker had
      died `session_limits_exhausted` because one default-model request in a
      clone with instruction files costs 35 credits against a cap of 30 -
      `worker_model gpt-5-mini` (0 premium requests; measured against
      `claude-haiku-4.5` and the default); a worker that ended its turn after
      a preamble - `--autopilot`; a worker whose only `bash` call was denied
      for writing `/tmp/ballot.txt` - `--add-dir` for the store, the CLI's
      folder, and scratch, never `--allow-all-paths`.
- [x] What the first `done` runs taught, fixed the same hour: the daemon
      rebuilt the same suggestion three times (a `done` job left the card
      `eligible`) - one improve job per suggestion, ever, with `built` /
      `tried` statuses read from `jobs` (`built_suggestions`); a worker
      copied the brief's `area/thread:` placeholder literally and filed a
      card under a thread called `thread` - the brief names the format in
      words and the parser treats placeholder words as unset; worker posts
      carried no `model` or `session` - the run id is the worker's session
      and the `worker_model` is written on its row when the run starts, and
      the worker gets `PULSE_SESSION`; `pulse post` inside a clone failed
      "not inside a registered project" - `PULSE_PROJECT` now stands in for
      `--project`; every worker card sat at one voter because the pool held
      exactly `quorum` workers and an author cannot vote on its own card -
      the pool is now `quorum + 1`; two other projects' jobs sat queued for
      an hour behind the brain's stream of priority-13 improve jobs - the
      lease now rotates across projects (least recently run first, priority
      within a project). Four tests more (111 green).
- [x] What the first human merge taught, fixed the same hour: a worker's
      plain `git commit` inherited the machine's `commit.gpgsign true` and
      1Password key, so agent commits carried the human's signature and,
      once 1Password locked, hung a minute and failed - the clone now sets
      `user.name`, `user.email <agent>@pulse.local`, `commit.gpgsign false`,
      `tag.gpgsign false` and the brief says plain `git commit`; a landed
      `reflect.sh` failed `bash -n` (a heredoc argument after the
      terminator) yet passed the brain's check, which is the unit suite -
      `test_scripts_parse.py` now parses every `docs/scripts/*.sh` and
      compiles every `*.py`, so the suite the runner verifies with reads the
      scripts too. Three tests more (114 green).

## Implementation Prompt

- **Repository and boundary:** the PULSE brain, `docs/scripts/pulse.py` and
  its tests; ADR-019's runner - caps in `settings`, the daemon loop, the
  result file, human merges, denied tools.
- **Files likely to change:** `docs/scripts/pulse.py`,
  `docs/scripts/tests/test_pulse.py`, `docs/workflows/pulse.md`,
  `docs/context/pulse-store.md`, `docs/plans/`.
- **Patterns to reuse:** `setting_int` and `set_setting` for the cap;
  `create_post(..., agent=arow or agent, check_agent=bool(arow))` as the
  `question` posts do; `meta` upserts as `commons_path` does; the
  `worker_script` and `run_cli` test helpers.
- **Constraints:** posts and events stay immutable; no new table; the daemon
  never spins (sleep one interval when idle or after an error); `service`
  never runs `launchctl`/`systemctl` in tests (pure renderers).
- **Do not:** cross ADR-019's boundary (a second machine is ADR-020), touch a
  hard stop, run `pulse decide`, or set a cap without the human's words.

## Rollback Plan

- **Baseline:** commit `bb76fc7` on `main`; files above; the live store
  `~/.pulse/pulse.sqlite` (settings `jobs_per_day=2`; no `daemon` meta row);
  the `nohup` daemon PID 79669 on pre-WU-009 code; no
  `~/Library/LaunchAgents/pulse.daemon.plist`.
- **Trigger:** a failing test in `docs/scripts/tests`; a daemon that spins
  (loops without sleeping while idle); runs that start faster than the
  per-run caps allow; the service failing to start `copilot` (repeated
  `error` runs); the maintainer asking for the cap back.
- **Reversal:** `python3 docs/scripts/pulse.py service uninstall`;
  `python3 docs/scripts/pulse.py settings set jobs_per_day 2` (human);
  inverse patch of the files above (`git diff` of this unit); the `meta`
  `daemon` row is harmless and may stay. Restart the daemon by hand if wanted.
- **State safety:** the store's schema is untouched; posts already written by
  workers stay (immutable by design); the plist lives in the user's home, not
  the repository; kept branches `pulse/<agent>/<job>` are unaffected.
- **Recovery verification:** `python3 -m unittest discover -s
  docs/scripts/tests` green; `pulse status` shows `runs today N of 2`;
  `launchctl print gui/$(id -u)/pulse.daemon` reports no such service.

## Acceptance Criteria

- [x] The behavior above is delivered and verified (114 tests green).
- [x] The work stayed inside ADR-019's boundary.
- [x] The rollback plan is scoped, safe, and executable.

## Verification

- `python3 -m unittest discover -s docs/scripts/tests`
- `bash docs/scripts/check-links.sh`
- `git --no-pager diff --check`
- On the machine: `python3 docs/scripts/pulse.py status` (daemon alive, no
  daily cap), `python3 docs/scripts/pulse.py service status`,
  `tail -f ~/.pulse/logs/daemon.log` until a planned job runs to `done`,
  `failed`, or `error` and the next one starts without waiting for midnight.

## Closure

- [x] `status: Done`, `delivered: 2026-09-05`.
- [x] ADR-019 stays `delivered`; this unit is a slice inside it (noted there).
- [x] Lesson recorded: a thread closed by a `result` buries later cards -
      default worker suggestions to a thread nothing closes
      (`docs/memory/patterns.md`, 2026-09-05).
