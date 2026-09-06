# Workflow - Remote Executors (for humans)

How a maintainer lends the runner a second machine's hands - a VM in a cloud
resource group, a box under a desk, any Linux host reachable over ssh - so
every registered project's jobs can run there while the store, the planner,
and the landing stay on this machine, the hub. Decided in
[ADR-020](../decisions/ADR-020-remote-executors.md); the plan is
[PLAN-020](../plans/remote-executors.md); the runner's own procedure is
[`pulse.md`](pulse.md) and the schema is
[`pulse-store.md`](../context/pulse-store.md). Every command below is the
human's by default: agents never add, activate, or remove an executor on
their own judgement, never provision a machine, and never place a login
(infrastructure, money, and credentials are hard stops in every tier). The
only exception is ADR-021's decided proposal gate for `pulse pair`: when
`PULSE_AGENT` is set, `pair` may run only if the store holds a human
`pulse decide ... yes`, no more than seven days old, on a proposal whose body
contains `pair: <executor>`. Credentials are never placed by an agent on its
own.

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
  work directory. Anything else is refused with exit 2. One verb, `key JOB
  EXECUTOR`, takes this run's private signing key on stdin after `prepare`
  and writes it as `<job>/run.key` (0600, outside the clone) with the
  clone's signing config and provenance hook
  ([ADR-022](../decisions/ADR-022-git-native-agent-identity.md)); `clean`
  removes it with the directory. A hub upgraded to that verb needs the hands
  refreshed once - `pulse pair <admin@host> --name <executor>` - or every
  run there ends `error` with `verb not allowed: key`.
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
op read 'op://Vault/item/field' | python3 docs/scripts/pulse.py executor login pulse-vm1 --admin pulseadmin@pulse-vm1 --with-token
# or, once and interactive on the machine:
ssh -t pulseadmin@pulse-vm1 sudo -iu pulse copilot login   # the hands' own login, not yours
```

Any other host works the same way when it has `git >= 2.30`, `python3`, the
copilot CLI with a login on the single-purpose user, `pulse-hands` on the
path, and exactly one `authorized_keys` line for that user:

```text
command="PULSE_HANDS_WORKDIR=/srv/pulse /usr/local/bin/pulse-hands",restrict ssh-ed25519 AAAA... pulse-hub
```

Narrow egress to what the CLI and the project's check need (GitHub and the
Copilot API endpoints, the package registries the verify command uses).

## Pair a machine that already exists

`pulse pair` is the second half of the Azure path for a Linux machine that
already exists:

```bash
python3 docs/scripts/pulse.py pair admin@host --name vm1 --dry-run
python3 docs/scripts/pulse.py pair admin@host --name vm1 --workdir /srv/pulse --workers 1 --projects '*'
```

The dry run prints every remote command and every local ssh config or
known-hosts change and changes nothing. The real run uses the human's admin
ssh session and passwordless `sudo`; it generates or reuses only the hub key
`~/.ssh/pulse_hands_ed25519`, never a personal key, inspects before any
change, and applies one idempotent setup: a locked `pulse` user,
`/usr/local/bin/pulse-hands`, a 0750 workdir, exactly one restricted
`authorized_keys` line, the sshd drop-in
`/etc/ssh/sshd_config.d/60-pulse-hands.conf`, git `commit.gpgsign false`,
and `core.askPass ""`. With `--install-copilot`, it installs Node.js 22 and
`@github/copilot` on apt systems only; on any other system that flag is a
refusal.

The sshd step is built to keep the admin path open. `pair` writes the drop-in,
runs `sshd -t`, and removes the drop-in without reloading when the check
fails. After a clean check it reloads sshd and opens a second admin
connection; if that connection fails, the held admin session removes the
drop-in and reloads again. Refusals exit 2 with nothing changed: non-Linux,
no passwordless sudo, an existing `authorized_keys` for `pulse` with any
other line, no `sshd_config.d`, a clone with a remote under the workdir, a
conflicting `Host <executor>` alias, or an executor of that name already tied
to another address.

After the remote side is safe, `pair` writes the local `Host <executor>` alias
with `IdentitiesOnly yes`, trusts the host key through the admin session, runs
`executor add <executor> --ssh pulse@<executor>`, and probes. The store sees
the alias, not the address.

The copilot login is a separate human credential step from the hub:

```bash
op read 'op://Vault/item/field' | python3 docs/scripts/pulse.py executor login vm1 --admin admin@host --with-token
python3 docs/scripts/pulse.py executor login vm1 --admin admin@host --remove
```

`executor login` reads one token from standard input only (`github_pat_`,
`gho_`, or `ghu_`; classic `ghp_` is refused because the CLI rejects it),
first writes the storage consent as user `pulse` - a headless machine has no
keychain, and the CLI's "store in plaintext?" question is answered no for
itself when standard input is a pipe, so the token would validate and then
be thrown away; `storeTokenPlaintext: true` in `~pulse/.copilot/settings.json`
(700/600, merged if the file has other keys) is that consent given in
advance, and the token stays home if this step fails - then pipes it to
`sudo -u pulse -H copilot login --with-token`, redacts token-shaped output,
records only `executor.login` with the executor name, token kind (`pat`,
`oauth`, `user`, or `removed`), and exit code, then probes; a passing probe
re-activates an executor the runner had drained for "not logged in".
`--remove` deletes `~pulse/.copilot` because the CLI has no logout, then
probes and leaves the executor drained. A `~/.copilot` holding only `logs/`
or only `settings.json` is not a login; the probe requires a login record in
`config.json` (non-empty `authTokens` or `loggedInUsers`) or a token in
`COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, or `GITHUB_TOKEN`.

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
| `logged_in` | `yes` - `~/.copilot/config.json` records a login (non-empty `authTokens` for a plaintext store or `loggedInUsers` for a keychain) or the CLI's token variable is set; a fresh machine, one with only `logs/` from a failed login, or one holding only the `settings.json` consent says `no` |
| `remotes` | `0` - no clone under the work directory has a remote (a leftover with one is a finding, not a job host) |
| `keys_ok` | `yes` - every `authorized_keys` line for that user carries `command=` and `restrict` |
| `disk_free_mb` | at least 1024 |
| `jobs` | job directories still present (`-` when none); informational - a passing probe sweeps the idle ones |
| `keys` | job directories still holding a `run.key` (`-` when none); one whose run is not `running` and that the sweep did not remove fails the probe (`N signing key(s) left without a live run`) |

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
- **Faults - the machine answered, and the answer means a human must fix
  it** ([WU-014](../plans/WU-014-executor-faults-are-the-machines.md)):
  hands that refuse a verb this hub needs (`verb not allowed: key` - they
  predate the hub; `pulse pair <admin@host> --name vm1` refreshes them) or
  a worker rejected at launch for its login (`re-authenticate`, `not logged
  in`, `Copilot Requests` … - `pulse executor login vm1 --admin
  <admin@host> --with-token`). The run ends `error` naming the command, the
  machine goes `drained` with `fault: …` in its `probe_note`
  (`executor.drained` event), and the job returns to the queue **with its
  attempt returned** - a machine fault never counts toward the three runner
  errors that fail a job - so the next loop runs it on `local` or on other
  hands. Only a human probe that passes (`probe`, `pair`, or `login`)
  re-activates the machine; the probe itself fails hands without a `keys=`
  field, so an upgraded hub names every machine left behind. On `local`
  there is no row to drain: a login failure there is charged like any
  runner error and shows in the daemon log.
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
  remove vm1`, then `pulse executor login vm1 --admin admin@host --remove`
  or remove the CLI folder as `pulse`; in Azure,
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
  compute on its own judgement. ADR-021 adds one narrow exception for
  `pulse pair`: an agent may run it only when the store holds the human's
  `pulse decide ... yes`, at most seven days old, on a proposal whose body
  carries the exact line `pair: <executor>`; the newest decision for that
  name wins and a later `no` closes it. That exception does not cover
  credentials, so `pulse executor login` is always the human's.
- Delete a resource group that holds anything you did not create, or retag
  one you found.
- Put a repository credential, a deploy key, or the store on the hands.
- Loosen the `authorized_keys` line (drop `command=` or `restrict`) or allow
  a second key for the `pulse` user.
- Treat a passing probe as proof of isolation: it checks the checklist it
  can see, not the cloud's network rules or the CLI's egress.
