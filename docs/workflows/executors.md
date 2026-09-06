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

## Provision a machine (Azure example)

`docs/scripts/hosted/azure-provision.sh` creates or updates one Ubuntu 24.04
VM from `docs/scripts/hosted/cloud-init.yaml`: `git`, `python3`, Node.js 22
and `@github/copilot`, the cloud's admin user with your key, a
single-purpose user `pulse` with `/srv/pulse`, `pulse-hands` installed at
`/usr/local/bin/pulse-hands`, and the hub's public key placed for `pulse`
with the forced command and `restrict`. Its network rule allows ssh from one
address (the hub's) only. It is idempotent - a rerun updates the ssh rule
when the hub's address changed and touches nothing else - and prints the
`pulse executor add` line at the end. The resource group is created when it
is missing and **used as it is when it exists**: never retagged, never
deleted, so the VM may live in a group shared with disposable work.
Costs: the VM (a `Standard_B2ms` is about 60 USD a month while it runs;
`az vm deallocate` stops the meter) and the Copilot premium requests the
worker's login spends inside `max_ai_credits` per run.

Who runs it: a human, or an agent only where the owner has recorded, for
that subscription, that agents may create compute (then the owner's naming
and tagging rules apply and `--tag key=value` carries them; tags name a
purpose, never a person). The copilot login on the VM is always a human's.

```bash
az account show --query name -o tsv                     # the subscription that pays
ssh-keygen -t ed25519 -N "" -C pulse-hub -f ~/.ssh/pulse_hands_ed25519   # the hub's own key, no passphrase: the daemon uses it
bash docs/scripts/hosted/azure-provision.sh \
  --group pulse-hands --location eastus --name pulse-vm1 --size Standard_B2ms \
  --pubkey ~/.ssh/pulse_hands_ed25519.pub --tag purpose=pulse-executor --tag ttl=persistent
```

Then give the hub a name for the machine and trust its host key through
Azure (read from the VM by the control plane, so the first connection is
never a blind one). The alias keeps the address out of the store, the
dashboard, and every board post:

```bash
IP="$(az vm show -g pulse-hands -n pulse-vm1 -d --query publicIps -o tsv)"
printf '\nHost pulse-vm1\n  HostName %s\n  IdentityFile ~/.ssh/pulse_hands_ed25519\n  IdentitiesOnly yes\n' "$IP" >> ~/.ssh/config
bash docs/scripts/hosted/azure-provision.sh --group pulse-hands --name pulse-vm1 --host-key >> ~/.ssh/known_hosts
ssh pulseadmin@pulse-vm1 'cloud-init status --wait'    # done when it prints status: done
ssh -t pulseadmin@pulse-vm1 sudo -iu pulse copilot login   # once, interactive; the hands' own login, not yours
```

Any other host works the same way when it has `git >= 2.30`, `python3`, the
copilot CLI with a login on the single-purpose user, `pulse-hands` on the
path, and exactly one `authorized_keys` line for that user:

```text
command="PULSE_HANDS_WORKDIR=/srv/pulse /usr/local/bin/pulse-hands",restrict ssh-ed25519 AAAA... pulse-hub
```

Narrow egress to what the CLI and the project's check need (GitHub and the
Copilot API endpoints, the package registries the verify command uses).

## Enroll, probe, run

```bash
python3 docs/scripts/pulse.py executor add vm1 --ssh pulse@vm1 --workdir /srv/pulse --workers 1 --projects '*'   # the ssh alias, not an address
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
| `logged_in` | `yes` - the CLI keeps its folder for that user; a fresh machine says `no` until `copilot login` ran there |
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
  remove vm1`, then on the VM `copilot logout` as `pulse`; in Azure,
  `bash docs/scripts/hosted/azure-provision.sh --group pulse-hands --name
  pulse-vm1 --delete` removes exactly what the script created (VM, disk,
  NIC, public IP, its NSG and VNET) and leaves the group; `az group delete`
  only when the group is yours and holds nothing else. Then drop the `Host`
  block from `~/.ssh/config` and the host key line
  (`ssh-keygen -R <address>`).

## Rollback

- **Trigger:** a run on an executor that lands a branch the hub did not
  fetch itself, a clone with a remote on the machine, a repository
  credential on it, a `stale` result landing, or a probe you cannot make
  pass.
- **Reversal:** `pulse executor drain vm1`, then `remove`; the store keeps
  the rows as history; `azure-provision.sh --delete` removes the VM and its
  own network pieces without touching the group. Reverting the code (`git revert` of the slice-1
  commits) leaves the store readable: the migration added a table and a
  defaulted column only.
- **Recovery check:** `pulse executor list` shows `local` alone; `pulse work
  --dry-run` prepares a local clone and lands nothing; the test suites pass
  (`python3 docs/scripts/tests/test_pulse.py`,
  `python3 -m unittest docs.scripts.tests.test_pulse_dashboard`).

## Never

- Let an agent run `pulse executor activate`, log the CLI in, or create
  compute where the owner has not recorded that agents may - the request is
  not the approval. Where the owner has, the agent follows the owner's
  naming and tagging rules and the probe still decides activation.
- Delete a resource group that holds anything you did not create, or retag
  one you found.
- Put a repository credential, a deploy key, or the store on the hands.
- Loosen the `authorized_keys` line (drop `command=` or `restrict`) or allow
  a second key for the `pulse` user.
- Treat a passing probe as proof of isolation: it checks the checklist it
  can see, not the cloud's network rules or the CLI's egress.
