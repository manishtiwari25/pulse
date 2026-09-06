#!/usr/bin/env bash
# pulse-hands: the forced command a PULSE executor answers with (ADR-020, PLAN-020 slice 1).
#
# The hub (the human's machine, where the store lives) reaches this machine over ssh as a single-purpose
# user whose authorized key is pinned to this script:
#
#   command="PULSE_HANDS_WORKDIR=/srv/pulse /usr/local/bin/pulse-hands",restrict ssh-ed25519 AAAA... pulse-hub
#
# Whatever the hub sends arrives in SSH_ORIGINAL_COMMAND as `b64 <base64 of NUL-separated argv>` (never
# eval'd); only the verbs below run, every path stays under the work directory, and the worker command must
# be the copilot CLI. This machine never pushes, never has a remote, never holds a token the hub did not put
# on it (the human places the login from the hub with `pulse executor login <name> --admin ... --with-token`,
# or runs `copilot login` here once).
#
# Verbs (one call each, from pulse.py's SshExecutor):
#   probe                          describe this machine: hands=ok user= workdir= git= copilot= logged_in= remotes= keys_ok= disk_free_mb= jobs= keys=
#   prepare JOB BRANCH AGENT       stdin: a git bundle; clones it into WORKDIR/JOB/clone, drops the remote, sets the agent's
#                                  identity and no signing yet; prints the base commit
#   key JOB EXECUTOR               stdin: this run's private ssh signing key (ADR-022); written to WORKDIR/JOB/run.key (0600),
#                                  the clone signs every commit with it and a clone-local hook adds the Pulse-Agent/
#                                  Pulse-Job/Pulse-Executor trailers; the key can only sign and leaves with `clean`
#   brief JOB                      stdin: the brief; written to WORKDIR/JOB/brief.md
#   run JOB MINUTES [K=V ...] -- CMD ...   run the worker in the clone with those env keys under a MINUTES timeout
#                                  (exit 124 when it ran out); only PULSE_* and GIT_TERMINAL_PROMPT keys, only copilot
#   result JOB                     print PULSE_RESULT.md (exit 3 when absent)
#   count JOB BASE                 commits since BASE
#   dirty JOB                      uncommitted tracked changes (porcelain)
#   changed JOB BASE               files changed since BASE
#   drop-result JOB                remove a committed PULSE_RESULT.md with one more commit; prints "dropped" or "clean"
#   verify JOB MINUTES -- CMD      run the project's own check (a shell string) in the clone under a timeout
#   clean JOB                      remove WORKDIR/JOB
#   git-upload-pack 'WORKDIR/JOB/clone'   what `git fetch ssh://...` sends; the hub pulls, this side never pushes
#
# Environment: PULSE_HANDS_WORKDIR (default /srv/pulse). PULSE_HANDS_ALLOW_ANY_WORKER=1 lets the test suite run
# a shell script as the worker; never set it in an authorized_keys line.

set -euo pipefail

WORKDIR="${PULSE_HANDS_WORKDIR:-/srv/pulse}"
RESULT_FILE="PULSE_RESULT.md"

die() { echo "pulse-hands: $*" >&2; exit 2; }

job_dir() {
  local job="$1"
  [[ "$job" =~ ^[A-Za-z0-9._-]+$ ]] || die "bad job id"
  [[ "$job" != "." && "$job" != ".." ]] || die "bad job id"
  printf '%s/%s' "$WORKDIR" "$job"
}

clone_dir() {
  local dir
  dir="$(job_dir "$1")/clone"
  [ -d "$dir/.git" ] || die "no clone for job $1"
  printf '%s' "$dir"
}

# timeout(1) on Linux; a perl alarm where coreutils is absent (macOS test runs). Exit 124 when time ran out.
# MINUTES is an integer; a trailing `s` makes it seconds (the test suite's short budgets).
with_timeout() {
  local budget="$1"; shift
  local seconds
  if [[ "$budget" =~ ^[0-9]+s$ ]]; then
    seconds="${budget%s}"
  elif [[ "$budget" =~ ^[0-9]+$ ]]; then
    seconds=$(( budget * 60 ))
  else
    die "bad minutes"
  fi
  [ "$seconds" -ge 1 ] || die "the budget must be at least one second"
  if command -v timeout >/dev/null 2>&1; then
    timeout --kill-after=30 "${seconds}s" "$@"
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout --kill-after=30 "${seconds}s" "$@"
  else
    # Like timeout(1): the worker gets its own process group so its children die with it - a grandchild that
    # outlived the budget would otherwise hold the pipes open and the caller would wait for it, not the budget.
    perl -e '
      use POSIX qw(WNOHANG);
      my $t = shift @ARGV;
      my $pid = fork();
      if (!defined $pid) { die "fork: $!"; }
      if ($pid == 0) { setpgrp(0, 0); exec @ARGV or die "exec: $!"; }
      local $SIG{ALRM} = sub {
        kill "TERM", -$pid;
        my $gone = 0;
        for (1..20) { if (waitpid($pid, WNOHANG) == $pid) { $gone = 1; last; } select(undef, undef, undef, 0.1); }
        kill "KILL", -$pid;
        waitpid($pid, 0) unless $gone;
        exit 124;
      };
      alarm $t;
      waitpid($pid, 0);
      my $rc = $? >> 8;
      $rc = 128 + ($? & 127) if ($? & 127);
      exit $rc;
    ' "$seconds" "$@"
  fi
}

verb_probe() {
  local remotes=0 gitv copv keys_ok logged_in free_mb d
  gitv="$(git --version 2>/dev/null | awk '{print $3}')"; gitv="${gitv:-missing}"
  if command -v copilot >/dev/null 2>&1; then
    copv="$(copilot --version 2>/dev/null | head -1 | sed 's/[^0-9.]//g; s/^\.*//; s/\.*$//')"; copv="${copv:-present}"
  else
    copv="missing"
  fi
  # A failed token login leaves only ~/.copilot/logs, and the login step writes settings.json before the token
  # arrives; count a login only when config.json records one (authTokens for a plaintext store, loggedInUsers for a
  # keychain) or when a token comes from an environment variable the CLI itself honors.
  logged_in="no"
  if [ "$copv" != "missing" ]; then
    if [ -n "${COPILOT_GITHUB_TOKEN:-}${GH_TOKEN:-}${GITHUB_TOKEN:-}" ]; then
      logged_in="yes"
    elif [ -s "$HOME/.copilot/config.json" ] && tr -d ' \t\r\n' < "$HOME/.copilot/config.json" | grep -q -E '"(authTokens":\{"|loggedInUsers":\[\{")'; then
      logged_in="yes"
    fi
  fi
  # The test suite's shell worker needs neither the CLI nor a login.
  if [ "${PULSE_HANDS_ALLOW_ANY_WORKER:-0}" = "1" ]; then copv="${copv/missing/test}"; logged_in="yes"; fi
  # Every clone under the work directory must have no remote (a leftover with one is a finding, not a job host).
  if [ -d "$WORKDIR" ]; then
    for d in "$WORKDIR"/*/clone; do
      [ -d "$d/.git" ] || continue
      if [ -n "$(git -C "$d" remote 2>/dev/null)" ]; then remotes=$(( remotes + 1 )); fi
    done
  fi
  # Every key that can reach this user must be pinned to this command and restricted.
  keys_ok="no"
  if [ -f "$HOME/.ssh/authorized_keys" ]; then
    local total pinned
    total="$(grep -cvE '^(#|[[:space:]]*$)' "$HOME/.ssh/authorized_keys" || true)"
    pinned="$(grep -cE '^command="[^"]*pulse-hands[^"]*",([^ ]*,)?restrict' "$HOME/.ssh/authorized_keys" || true)"
    if [ "${total:-0}" -gt 0 ] && [ "${total:-0}" = "${pinned:-0}" ]; then keys_ok="yes"; fi
  fi
  [ -n "${PULSE_HANDS_KEYS_OK:-}" ] && keys_ok="$PULSE_HANDS_KEYS_OK"
  mkdir -p "$WORKDIR" 2>/dev/null || true
  free_mb="$(df -Pm "$WORKDIR" 2>/dev/null | awk 'NR==2 {print $4}')"; free_mb="${free_mb:-0}"
  # Job directories still here (a run the hub lost, a hub that died mid-job): named so the hub can clean them.
  local jobs="" j
  for d in "$WORKDIR"/*/; do
    [ -d "$d" ] || continue
    j="$(basename "$d")"
    jobs="${jobs:+$jobs,}$j"
  done
  # Job directories still holding a run's signing key: the hub checks each against its live runs (ADR-022).
  local keys=""
  for d in "$WORKDIR"/*/; do
    [ -f "${d}run.key" ] || continue
    j="$(basename "$d")"
    keys="${keys:+$keys,}$j"
  done
  printf 'hands=ok user=%s workdir=%s git=%s copilot=%s logged_in=%s remotes=%s keys_ok=%s disk_free_mb=%s jobs=%s keys=%s\n' \
    "$(id -un)" "$WORKDIR" "$gitv" "$copv" "$logged_in" "$remotes" "$keys_ok" "$free_mb" "${jobs:--}" "${keys:--}"
}

verb_prepare() {
  local job="$1" branch="$2" agent="$3" dir bundle
  [[ "$agent" =~ ^[A-Za-z0-9._-]+$ ]] || die "bad agent name"
  [[ "$branch" =~ ^[A-Za-z0-9._/-]+$ ]] || die "bad branch"
  dir="$(job_dir "$job")"
  rm -rf "$dir"
  mkdir -p "$dir"
  bundle="$dir/source.bundle"
  cat > "$bundle"
  # Older gits (2.43 on Ubuntu 24.04 among them) refuse `git bundle verify` outside a repository, and the
  # forced command starts in the hands' home, which is none: verify from an empty scratch repository.
  git init -q "$dir/verify" || die "cannot make the scratch repository"
  git -C "$dir/verify" bundle verify "$bundle" >/dev/null 2>&1 || { rm -rf "$dir/verify"; die "the bundle does not verify"; }
  rm -rf "$dir/verify"
  if [ "$branch" = "HEAD" ]; then
    git -c advice.detachedHead=false clone -q --no-tags "$bundle" "$dir/clone"
  else
    git clone -q --no-tags -b "$branch" "$bundle" "$dir/clone"
  fi
  rm -f "$bundle"
  git -C "$dir/clone" remote remove origin 2>/dev/null || true
  git -C "$dir/clone" config user.name "$agent"
  git -C "$dir/clone" config user.email "$agent@pulse.local"
  git -C "$dir/clone" config commit.gpgsign false
  git -C "$dir/clone" config tag.gpgsign false
  git -C "$dir/clone" rev-parse HEAD
}

verb_key() {
  # ADR-022: the hub made this key for this run alone. It lands next to the clone, never inside it, readable by
  # this user only; the clone pins every knob the signature depends on so no machine-wide signer or hooks
  # directory can take over, and the hook stamps who, which job, which hands on every commit.
  local job="$1" executor="$2" dir clone key hooks agent
  [[ "$executor" =~ ^[A-Za-z0-9._-]+$ ]] || die "bad executor name"
  dir="$(job_dir "$job")"
  clone="$(clone_dir "$job")"
  key="$dir/run.key"
  umask 077
  cat > "$key"
  chmod 600 "$key"
  if ! grep -q -- '-----BEGIN OPENSSH PRIVATE KEY-----' "$key" 2>/dev/null; then
    rm -f "$key"
    die "not an openssh private key"
  fi
  agent="$(git -C "$clone" config user.name)"
  [[ "$agent" =~ ^[A-Za-z0-9._-]+$ ]] || die "the clone has no agent identity (prepare first)"
  hooks="$clone/.git/hooks"
  git -C "$clone" config gpg.format ssh
  git -C "$clone" config gpg.ssh.program ssh-keygen
  git -C "$clone" config user.signingkey "$key"
  git -C "$clone" config commit.gpgsign true
  git -C "$clone" config tag.gpgsign false
  git -C "$clone" config core.hooksPath "$hooks"
  mkdir -p "$hooks"
  {
    printf '#!/bin/sh\n'
    printf '# pulse: provenance trailers for every commit of this run (ADR-022). Local to this clone, never committed.\n'
    printf "exec git interpret-trailers --in-place --if-exists doNothing --trailer 'Pulse-Agent: %s' --trailer 'Pulse-Job: %s' --trailer 'Pulse-Executor: %s' \"\$1\"\n" "$agent" "$job" "$executor"
  } > "$hooks/prepare-commit-msg"
  chmod 755 "$hooks/prepare-commit-msg"
  echo keyed
}

verb_brief() {
  local dir
  dir="$(job_dir "$1")"
  [ -d "$dir" ] || die "no job dir for $1 (prepare first)"
  cat > "$dir/brief.md"
}

verb_run() {
  local job="$1" minutes="$2"; shift 2
  local clone kv rc
  clone="$(clone_dir "$job")"
  while [ $# -gt 0 ] && [ "$1" != "--" ]; do
    kv="$1"; shift
    [[ "$kv" =~ ^(PULSE_[A-Z_]+|GIT_TERMINAL_PROMPT)=.*$ ]] || die "env key not allowed: ${kv%%=*}"
    export "${kv?}"
  done
  [ "${1:-}" = "--" ] || die "run needs -- before the worker command"
  shift
  [ $# -gt 0 ] || die "run needs a worker command"
  if [ "${PULSE_HANDS_ALLOW_ANY_WORKER:-0}" != "1" ]; then
    [ "$(basename -- "$1")" = "copilot" ] || die "only the copilot worker may run here"
  fi
  cd "$clone"
  # The forced-command request must not leak into the worker: a test suite that spawns these hands would
  # otherwise re-run the ssh request instead of its own argv.
  unset SSH_ORIGINAL_COMMAND SSH_CONNECTION SSH_CLIENT
  set +e
  with_timeout "$minutes" "$@"
  rc=$?
  set -e
  exit "$rc"
}

verb_result() {
  local clone
  clone="$(clone_dir "$1")"
  [ -f "$clone/$RESULT_FILE" ] || exit 3
  cat "$clone/$RESULT_FILE"
}

verb_count() {
  local clone base="$2"
  clone="$(clone_dir "$1")"
  [[ "$base" =~ ^[0-9a-f]{40,64}$ ]] || die "bad base"
  git -C "$clone" rev-list --count "$base..HEAD"
}

verb_dirty() {
  local clone
  clone="$(clone_dir "$1")"
  git -C "$clone" status --porcelain --untracked-files=no
}

verb_changed() {
  local clone base="$2"
  clone="$(clone_dir "$1")"
  [[ "$base" =~ ^[0-9a-f]{40,64}$ ]] || die "bad base"
  git -C "$clone" diff --name-only "$base..HEAD"
}

verb_drop_result() {
  local clone
  clone="$(clone_dir "$1")"
  if git -C "$clone" ls-files --error-unmatch "$RESULT_FILE" >/dev/null 2>&1; then
    git -C "$clone" rm -q --cached "$RESULT_FILE"
    git -C "$clone" commit -q -m "runner: drop $RESULT_FILE (never committed)"
    echo dropped
  else
    echo clean
  fi
}

verb_verify() {
  local job="$1" minutes="$2"; shift 2
  local clone rc
  [ "${1:-}" = "--" ] || die "verify needs -- before the command"
  shift
  [ $# -eq 1 ] || die "verify takes one shell command string"
  clone="$(clone_dir "$job")"
  cd "$clone"
  unset SSH_ORIGINAL_COMMAND SSH_CONNECTION SSH_CLIENT
  set +e
  with_timeout "$minutes" bash -c "$1"
  rc=$?
  set -e
  exit "$rc"
}

verb_clean() {
  local dir
  dir="$(job_dir "$1")"
  rm -rf "$dir"
}

verb_upload_pack() {
  # git sends: git-upload-pack '/srv/pulse/JOB/clone'
  local path="$1"
  path="${path#\'}"; path="${path%\'}"
  case "$path" in
    "$WORKDIR"/*/clone) ;;
    *) die "upload-pack path must be a job clone under $WORKDIR" ;;
  esac
  [[ "$path" != *".."* ]] || die "bad path"
  [ -d "$path/.git" ] || die "no such clone"
  exec git upload-pack "$path"
}

main() {
  local verb="${1:-}"; shift || true
  case "$verb" in
    probe)        verb_probe ;;
    prepare)      [ $# -eq 3 ] || die "prepare JOB BRANCH AGENT"; verb_prepare "$@" ;;
    key)          [ $# -eq 2 ] || die "key JOB EXECUTOR"; verb_key "$@" ;;
    brief)        [ $# -eq 1 ] || die "brief JOB"; verb_brief "$@" ;;
    run)          [ $# -ge 4 ] || die "run JOB MINUTES [K=V ...] -- CMD ..."; verb_run "$@" ;;
    result)       [ $# -eq 1 ] || die "result JOB"; verb_result "$@" ;;
    count)        [ $# -eq 2 ] || die "count JOB BASE"; verb_count "$@" ;;
    dirty)        [ $# -eq 1 ] || die "dirty JOB"; verb_dirty "$@" ;;
    changed)      [ $# -eq 2 ] || die "changed JOB BASE"; verb_changed "$@" ;;
    drop-result)  [ $# -eq 1 ] || die "drop-result JOB"; verb_drop_result "$@" ;;
    verify)       [ $# -ge 4 ] || die "verify JOB MINUTES -- CMD"; verb_verify "$@" ;;
    clean)        [ $# -eq 1 ] || die "clean JOB"; verb_clean "$@" ;;
    git-upload-pack) [ $# -eq 1 ] || die "git-upload-pack PATH"; verb_upload_pack "$@" ;;
    "")           die "no command (this key only runs pulse-hands verbs)" ;;
    *)            die "verb not allowed: $verb" ;;
  esac
}

# Under a forced command the real request is in SSH_ORIGINAL_COMMAND. It is never eval'd: the hub sends
# `b64 <base64 of NUL-separated argv>` (pulse.py ssh_command), and git's own fetch sends exactly
# `git-upload-pack '<path>'`. Anything else is refused.
if [ -n "${SSH_ORIGINAL_COMMAND:-}" ]; then
  if [[ "$SSH_ORIGINAL_COMMAND" =~ ^git-upload-pack\ \'([^\'[:cntrl:]]+)\'$ ]]; then
    set -- git-upload-pack "${BASH_REMATCH[1]}"
  elif [[ "$SSH_ORIGINAL_COMMAND" =~ ^b64\ ([A-Za-z0-9+/=]+)$ ]]; then
    ARGV=()
    while IFS= read -r -d '' item; do ARGV+=("$item"); done < <(printf '%s' "${BASH_REMATCH[1]}" | base64 --decode)
    [ "${#ARGV[@]}" -gt 0 ] || die "empty command"
    set -- "${ARGV[@]}"
  else
    die "refused: commands arrive as 'b64 ...' from pulse.py or as git-upload-pack"
  fi
fi
main "$@"
