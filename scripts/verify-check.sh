#!/bin/sh
# Daily project check: run the project's own verification in this throwaway
# clone (fmt, clippy, workspace tests). The runner passes PULSE_HOME (the
# daemon state dir) and PULSE_SCHEDULE_ID (this schedule's id) in the
# environment; neither holds a secret.
#
# Keep the check's Cargo target dir in a stable lane under the daemon's
# cache instead: per schedule when the runner names one, else one shared
# daily-check lane. The lane persists across the daily runs, so a run only
# rebuilds what the last day of commits changed, and it never overlaps the
# `cargo-target` lane the landing verify and job clones use (job lanes are
# pruned by the sweep while a run holds them; this lane is only touched by
# this schedule's own runs, which the daemon serializes).
#
# Timeout shape (schedule.script_timeout_minutes = 20): two suites are slow
# because their tests wait on real-time polling loops and fixtures rather
# than burn CPU: pulse-evolve (~320s serial) and pulse-inspect (~230s
# serial, ~190s of it one 400x25ms polling test). `cargo test --workspace`
# walks suites serially and needs ~25m wall here, so the daily check kept
# dying at the 20-minute script limit. We compile once, then run those two
# suites as one test per process through a bounded worker pool while the
# rest of the workspace runs serially alongside (~4m). Total wall ~8m. New
# tests are picked up automatically (the pool queue is built from `--list`
# at run time) and every test asserts exactly what it asserted before: only
# scheduling changes, never what is verified.
# The same script backs the registered land-time verify command: a
# land-time invocation exports VERIFY_TARGET_DIR (the runner's warm
# cargo-target-land lane) and the script then runs everything in that lane
# instead of the daily-check one, so the rule-4 `cargo test --workspace` at
# land time gets the same pooled heavy suites instead of ~25m serial, so a
# landing no longer eats a third of an agent run's 120m cap. Without
# VERIFY_TARGET_DIR the script keeps its daily-check lane behavior.
set -eu
if [ -n "${VERIFY_TARGET_DIR:-}" ]; then
    CARGO_TARGET_DIR=$VERIFY_TARGET_DIR
else
    case ${PULSE_HOME:-} in
    "") root=target ;;
    *) root=$PULSE_HOME/cache/cargo-target-check ;;
    esac
    CARGO_TARGET_DIR=$root/amistio-template/${PULSE_SCHEDULE_ID:-daily-check}
fi
export CARGO_TARGET_DIR
mkdir -p "$CARGO_TARGET_DIR"

POOL=${VERIFY_POOL:-6}
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# Compile every test binary once; nothing below recompiles. Capture the
# output: on any change cargo lists each test binary as
# `Executable unittests ... (PATH)`, and that path (hash included) is the one
# authority for what we just built. A bare glob over the deps dir could pick
# a stale binary from an earlier hash, which a busy store then hangs.
LOG_DIR=$(mktemp -d "${TMPDIR:-/tmp}/verify-logs.XXXXXX")
trap 'rm -rf "$LOG_DIR"' EXIT
COMPILE_OUT="$LOG_DIR/compile.out"
cargo test --workspace --no-run 2>&1 | tee "$COMPILE_OUT" | tail -5

DEPS="$CARGO_TARGET_DIR/debug/deps"

# The exact test binary of one crate: the Executable lines of the compile
# above when they are there. When the compile was a no-op (everything fresh)
# those lines are absent, so re-resolve with `--message-format=json`, which
# always emits one compiler-artifact line per target, executable included.
# Falling back to a glob over the deps dir could pick a stale binary from an
# earlier hash, which a busy store then hangs.
bin_for() {
    hit=$(sed -n 's/.*( *\([^()]*\)) *$/\1/p' "$COMPILE_OUT" |
        grep "/$1-[0-9a-f][0-9a-f]*$" | head -1)
    if [ -z "$hit" ]; then
        hit=$(cargo test --workspace --no-run --message-format=json 2>/dev/null |
            tr ',' '\n' | sed -n 's/.*"executable":"\([^"]*\)".*/\1/p' |
            grep "/$1-[0-9a-f][0-9a-f]*$" | head -1)
    fi
    if [ -n "$hit" ] && [ -x "$hit" ]; then
        echo "$hit"
        return 0
    fi
    echo "verify-check: cannot resolve the $1 test binary in $DEPS" >&2
    return 1
}

EVOLVE_BIN=$(bin_for pulse_evolve)
INSPECT_BIN=$(bin_for pulse_inspect)

echo "verify-check: evolve suite: $EVOLVE_BIN"
echo "verify-check: inspect suite: $INSPECT_BIN"

EVOLVE_LIST="$LOG_DIR/evolve.list"
INSPECT_LIST="$LOG_DIR/inspect.list"

"$EVOLVE_BIN" --list --format terse 2>/dev/null | sed 's/: test$//' >"$EVOLVE_LIST"
"$INSPECT_BIN" --list --format terse 2>/dev/null | sed 's/: test$//' >"$INSPECT_LIST"
[ -s "$EVOLVE_LIST" ] || {
    echo "verify-check: no pulse-evolve tests listed" >&2
    exit 1
}
[ -s "$INSPECT_LIST" ] || {
    echo "verify-check: no pulse-inspect tests listed" >&2
    exit 1
}

# Worker: each queue line is BIN<TAB>TEST; run it, keep the log.
worker() {
    rc=0
    while IFS="$(printf '\t')" read -r bin name; do
        [ -n "$name" ] || continue
        log="$LOG_DIR/$(echo "$name" | tr ': ' '__').log"
        if ! "$bin" --exact "$name" >"$log" 2>&1; then
            echo "FAILED: $name"
            rc=1
        fi
    done
    return $rc
}

# Work queue: every evolve test, then every inspect test. Split it
# round-robin across workers so the slow tests (which sit together in each
# suite's list) spread across workers instead of stacking in one chunk.
QUEUE="$LOG_DIR/queue"
: >"$QUEUE"
while read -r t; do
    printf '%s\t%s\n' "$EVOLVE_BIN" "$t" >>"$QUEUE"
done <"$EVOLVE_LIST"
while read -r t; do
    printf '%s\t%s\n' "$INSPECT_BIN" "$t" >>"$QUEUE"
done <"$INSPECT_LIST"

PIDS="$LOG_DIR/pids"
: >"$PIDS"
i=0
while [ "$i" -lt "$POOL" ]; do
    awk -v n="$POOL" -v k="$i" 'NR % n == k' "$QUEUE" >"$LOG_DIR/part.$i"
    worker <"$LOG_DIR/part.$i" &
    echo $! >>"$PIDS"
    i=$((i + 1))
done

# The rest of the workspace (both pulse targets included) runs serially in
# the foreground. The two heavy suites are excluded at the test-runner level
# (`cargo test -p`) rather than with a test-name `--skip` filter, because
# `--skip pulse_inspect` only matches test names, not the suite: every
# pulse-inspect test would still run a second time here, 192s of it on the
# critical path. `-p` selects the suite, so these two crates' unit tests run
# exactly once, in the pool above.
REST_RC=0
cargo test --workspace \
    --exclude pulse-evolve --exclude pulse-inspect || REST_RC=$?
# The pulse package itself (its lib unit tests) is part of --workspace and
# stays here; both of its targets are quick.

POOL_RC=0
while read -r p; do
    wait "$p" || POOL_RC=1
done <"$PIDS"

# Surface failed test output before exiting red.
if [ "$POOL_RC" -ne 0 ] || [ "$REST_RC" -ne 0 ]; then
    echo "verify-check: test failures" >&2
    for f in "$LOG_DIR"/*.log; do
        [ -f "$f" ] || continue
        if grep -q 'panicked\|FAILED' "$f" 2>/dev/null; then
            echo "--- $f" >&2
            tail -40 "$f" >&2
        fi
    done
    exit 1
fi

echo "verify-check: fmt, clippy and workspace tests green"
