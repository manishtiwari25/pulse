# Workflow - Remote Executors (for humans)

How a maintainer lends the runner a second machine's hands - a VM in a cloud
resource group, a box under a desk, any Linux host reachable over ssh - so
every registered project's jobs can run there while the store, the planner,
and the landing stay on this machine, the hub. Decided in
[ADR-020](../decisions/ADR-020-remote-executors.md); the plan is
[PLAN-020](../plans/remote-executors.md); the runner's own procedure is
[`pulse.md`](pulse.md) and the schema is
[`pulse-store.md`](../context/pulse-store.md). Every command below is the
human's: agents never add, activate, or remove an executor, never provision a
machine, and never place a login (infrastructure, money, and credentials are
hard stops in every tier).

## The shape

- **One brain, many hands.** The hub keeps `~/.pulse/pulse.sqlite`, plans
  jobs, casts the workers' votes, and lands kept commits on
  `pulse/<agent>/<job>` branches in the real checkouts. An executor only
  clones a bundle the hub sends, runs one worker under the same caps and
  deny list, runs the project's own check, and hands the commits back when
  the hub fetches them. It never pushes, never has a remote, never holds a
  repository credential, and never sees the store.
- **One program on the wire.** The hub's key on the executor is bound to a
  forced command, `pulse-hands` (`docs/scripts/hosted/pulse-hands.sh`),
  with `restrict` (no agent, port, or X11 forwarding, no pty). It accepts a
  fixed list of verbs whose argv arrives as base64 - never a shell string,
  never `eval` - plus git's own `git-upload-pack` for a job clone under its
  work directory. Anything else is refused with exit 2.
- **Same rules everywhere.** Tier 0/1 jobs only, `--deny-tool` for `git
  push`, `git remote`, `gh`, no built-in MCP servers, `--no-ask-user`,
  `--max-ai-credits`, the wall-time cap, owner-owned paths refused at
  landing, a committed `PULSE_RESULT.md` dropped before landing.
  `jobs_per_day` counts every executor together.

## Provision a machine (Azure example, human-run)

`docs/scripts/hosted/azure-provision.sh` creates or updates one resource
group with one Ubuntu 24.04 VM from `docs/scripts/hosted/cloud-init.yaml`:
`git`, `python3`, Node.js 22 and `@github/copilot`, a single-purpose user
`pulse` with `/srv/pulse`, `pulse-hands` installed at
`/usr/local/bin/pulse-hands`, and the hub's public key placed with the
forced command and `restrict`. Its network rule allows ssh from one address
(yours) only. It is idempotent and prints the `pulse executor add` line at
the end. Costs are yours: the VM (a `Standard_B2ms` is about 60 USD a month
while it runs; `az vm deallocate` stops the meter) and the Copilot premium
requests the worker's login spends inside `max_ai_credits` per run.

```bash
az login && az account show --query name -o tsv        # the subscription that pays
bash docs/scripts/hosted/azure-provision.sh \
  --group pulse-hands --location eastus --name pulse-vm1 \
  --size Standard_B2ms --pubkey ~/.ssh/id_ed25519.pub   # the hub's key; --allow-ip defaults to your address
ssh -t pulse-admin@<ip> sudo -iu pulse copilot login    # once, interactive; the hands' own login, not yours
```

Any other host works the same way when it has `git >= 2.30`, `python3`, the
copilot CLI with a login on the single-purpose user, `pulse-hands` on the
path, and exactly one `authorized_keys` line for that user:

```text
command="PULSE_HANDS_WORKDIR=/srv/pulse /usr/local/bin/pulse-hands",restrict ssh-ed25519 AAAA... hub
```

Narrow egress to what the CLI and the project's check need (GitHub and the
Copilot API endpoints, the package registries the verify command uses).

## Enroll, probe, run

```bash
python3 docs/scripts/pulse.py executor add vm1 --ssh pulse@<ip> --workdir /srv/pulse --workers 1 --projects '*'
python3 docs/scripts/pulse.py executor probe vm1      # activates it when every check passes
python3 docs/scripts/pulse.py executor list           # local + vm1, status, slots, runs, last probe
python3 docs/scripts/pulse.py work --dry-run --executor vm1   # bundle over, clone there, brief written, no worker, cleaned
python3 docs/scripts/pulse.py work --executor vm1     # one real job there
python3 docs/scripts/pulse.py status                  # the executors line the daemon also reads
```

`add` registers the machine **drained**: it takes no job until a probe
passes. The probe connects with `BatchMode=yes`, `ConnectTimeout=15`, and
`ClearAllForwardings=yes` and asks the hands to report; it passes only when

| Field | Must be |
| --- | --- |
| `hands` | `ok` - the forced command answered |
| `user` | the single-purpose user |
| `workdir` | exactly the `--workdir` you registered (absolute) |
| `git`, `copilot` | present with a version (`missing` fails) |
| `logged_in` | not `no` - the CLI's login exists for that user |
| `remotes` | `0` - no clone under the work directory has a remote (a leftover with one is a finding, not a job host) |
| `keys_ok` | `yes` - every `authorized_keys` line for that user carries `command=` and `restrict` |
| `disk_free_mb` | at least 1024 |
| `jobs` | job directories still present (`-` when none); informational - a passing probe sweeps the idle ones |

The note is stored (`probe_note`) and shown by `list`, `status`, and the
dashboard's Agents page. `activate` refuses an executor whose last probe
failed; run `probe` again after fixing the machine. The daemon needs no
restart: it reads the executors each loop and fills every free slot
(remote hands limited to some projects lease first, `local` is the
fallback), running the leased jobs in parallel and printing one line per
run, `done <job> <agent> on vm1: <summary>`.

## Watch, drain, remove

- **Daemon log** (`~/.pulse/logs/daemon.log`): `error <job> … on vm1:
  executor vm1 unreachable before worker start` means ssh failed before the
  worker began - the job is requeued and nothing was spent; `failed … on
  vm1: executor vm1 unreachable after worker start` means contact was lost
  mid-run - the cap was spent and the job fails after its attempts run out;
  `executor vm1 is reachable again: ok: …` closes an outage.
- **Unreachable.** Three transport errors in a row set `status =
  unreachable`; the daemon skips the machine and re-probes with a doubling
  wait (1, 2, 4 minutes … capped at its interval or an hour). A passing
  probe - the daemon's or yours - brings it back with the counters reset.
- **Drain** (`pulse executor drain vm1`) stops new leases and lets the
  running job finish. **Remove** (`pulse executor remove vm1`) is refused
  while a run is on it; drain first. Both are events; the machine's runs
  stay in `runs` with `executor = vm1`.
- **A VM dies mid-run.** The hub's run ends `failed` (contact lost after
  the worker started) or `error` (before); a run left `running` by a dead
  *hub* is closed as `lost` the next time the daemon starts. Nothing landed
  either way - the hub lands only what it fetched and verified. When the
  machine returns, the probe reports the job directories still there
  (`jobs=`), and a passing probe removes every one that no running run
  holds (`jobs=swept:N` in the note, one `executor.sweep` event each); a
  clone with a remote fails the probe instead and is yours to inspect.
- **Retire the machine.** `pulse executor drain vm1`, wait, `pulse executor
  remove vm1`, then on the VM remove the hub's key from `authorized_keys`
  and `copilot logout` as `pulse`; in Azure, `az group delete --name
  pulse-hands` removes the VM, disk, and network - your command, never an
  agent's.

## Rollback

- **Trigger:** a run on an executor that lands a branch the hub did not
  fetch itself, a clone with a remote on the machine, a repository
  credential on it, a `stale` result landing, or a probe you cannot make
  pass.
- **Reversal:** `pulse executor drain vm1`, then `remove`; the store keeps
  the rows as history. Reverting the code (`git revert` of the slice-1
  commits) leaves the store readable: the migration added a table and a
  defaulted column only.
- **Recovery check:** `pulse executor list` shows `local` alone; `pulse work
  --dry-run` prepares a local clone and lands nothing; the test suites pass
  (`python3 docs/scripts/tests/test_pulse.py`,
  `python3 -m unittest docs.scripts.tests.test_pulse_dashboard`).

## Never

- Let an agent run `pulse executor add|activate|remove`, provision a
  machine, place a key, or log the CLI in - the request is not the approval.
- Put a repository credential, a deploy key, or the store on the hands.
- Loosen the `authorized_keys` line (drop `command=` or `restrict`) or allow
  a second key for the `pulse` user.
- Treat a passing probe as proof of isolation: it checks the checklist it
  can see, not the cloud's network rules or the CLI's egress.
