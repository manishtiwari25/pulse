#!/usr/bin/env bash
# Prepare and activate the two private held-out fixtures required by ADR-012.
#
# The maintainer chooses an external directory. This helper assigns H01/H02,
# creates private templates without overwriting content, validates them
# without printing their values, and updates only the existing Held out: and
# loop: gate lines. It never commits, pushes, removes STOP, or writes a
# baseline.
#
# Usage:
#   docs/scripts/evolution-setup.sh status [--store PATH] [--repo PATH]
#   docs/scripts/evolution-setup.sh prepare --store PATH [--repo PATH]
#   docs/scripts/evolution-setup.sh activate --store PATH [--repo PATH]
set -eu

usage() {
    cat <<'EOF'
Usage:
  docs/scripts/evolution-setup.sh status [--store PATH] [--repo PATH]
  docs/scripts/evolution-setup.sh prepare --store PATH [--repo PATH]
  docs/scripts/evolution-setup.sh activate --store PATH [--repo PATH]

status    Read the repository gates and, when --store is supplied, validate
          H01.md and H02.md without printing their contents. Exits 0 when the
          gates are active, 1 otherwise; never writes.
prepare   Create private H01.md and H02.md templates outside the repository.
          Existing files are never overwritten; while the set is pending,
          fixtures older than that decision are retired (renamed, never
          deleted) so the templates are fresh.
activate  Validate both private fixtures, the baseline, STOP, and the gate
          files, then set "Held out: H01, H02" and "loop: on". No commit is
          created.

  --store PATH  Maintainer-chosen private directory outside the repository
  --repo PATH   Repository root (default: the repository containing this script)
  -h, --help    Show this help
EOF
}

fail() {
    echo "evolution-setup: $1" >&2
    exit "${2:-1}"
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd -P)"
COMMAND=""
STORE=""

[ $# -ge 1 ] || { usage >&2; exit 2; }
case "$1" in
    status|prepare|activate) COMMAND="$1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac

while [ $# -gt 0 ]; do
    case "$1" in
        --store) [ $# -ge 2 ] || { usage >&2; exit 2; }; STORE="$2"; shift 2 ;;
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd -P)"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

case "$COMMAND" in
    prepare|activate) [ -n "$STORE" ] || fail "--store is required for $COMMAND" 2 ;;
esac

EVO="$REPO/docs/evolution"
CONST="$EVO/constitution.md"
EVAL="$EVO/evaluators.md"
ARCH="$EVO/archive.md"
STOP="$EVO/STOP"
STORE_REAL=""

frontmatter_value() {
    [ -f "$1" ] || return 0
    awk -v key="$2" '
        NR == 1 && !/^---/ { exit }
        /^---/ { c++; if (c == 2) exit; next }
        c == 1 && index($0, key ":") == 1 {
            v = substr($0, length(key) + 2)
            gsub(/^[ \t]+|[ \t]+$/, "", v)
            print v
            exit
        }
    ' "$1"
}

held_out_value() {
    [ -f "$EVAL" ] || return 0
    awk '/^Held out:/ {
        sub(/^Held out:[ \t]*/, "")
        gsub(/[ \t]+$/, "")
        print
        exit
    }' "$EVAL"
}

line_count() {
    awk -v prefix="$2" 'index($0, prefix) == 1 { n++ } END { print n + 0 }' "$1"
}

archive_has_baseline() {
    [ -n "$1" ] && [ "$1" != "pending" ] && [ -f "$ARCH" ] || return 1
    awk -F'|' -v wanted="$1" '
        function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
        /^\|/ && trim($2) == wanted { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$ARCH"
}

file_mode() {
    stat -f '%Lp' "$1" 2>/dev/null || stat -c '%a' "$1" 2>/dev/null
}

resolve_store() {
    [ -n "$STORE" ] || return 0

    probe="$STORE"
    while [ ! -d "$probe" ]; do
        parent="$(dirname "$probe")"
        [ "$parent" != "$probe" ] || fail "cannot resolve --store $STORE" 2
        probe="$parent"
    done
    probe_real="$(cd "$probe" && pwd -P)"
    case "$probe_real/" in
        "$REPO"/*) fail "refusing --store inside the repository ($STORE)" 2 ;;
    esac

    if [ "$COMMAND" = "prepare" ]; then
        [ ! -e "$STORE" ] || [ -d "$STORE" ] || fail "--store is not a directory ($STORE)" 2
        if [ ! -d "$STORE" ]; then
            old_umask="$(umask)"
            umask 077
            mkdir -p "$STORE"
            umask "$old_umask"
            chmod 700 "$STORE"
        fi
    else
        [ -d "$STORE" ] || fail "no private fixture directory at $STORE" 2
    fi

    STORE_REAL="$(cd "$STORE" && pwd -P)"
    case "$STORE_REAL/" in
        "$REPO"/*) fail "refusing --store inside the repository ($STORE_REAL)" 2 ;;
    esac
}

field_count() {
    awk -v key="$2" 'index($0, key ":") == 1 { n++ } END { print n + 0 }' "$1"
}

field_value() {
    awk -v key="$2" 'index($0, key ":") == 1 {
        v = substr($0, length(key) + 2)
        gsub(/^[ \t]+|[ \t]+$/, "", v)
        print v
        exit
    }' "$1"
}

validate_fixture() {
    file="$1"
    id="$2"

    [ -f "$file" ] && [ ! -L "$file" ] || {
        echo "evolution-setup: $id must be a regular file, not a symlink" >&2
        return 1
    }
    mode="$(file_mode "$file")" || {
        echo "evolution-setup: cannot read permissions for $id" >&2
        return 1
    }
    perm=$((8#$mode))
    if [ $((perm & 077)) -ne 0 ]; then
        echo "evolution-setup: $id permissions must not allow group or other access (use chmod 600)" >&2
        return 1
    fi

    for key in tier request expected checks; do
        count="$(field_count "$file" "$key")"
        if [ "$count" -ne 1 ]; then
            echo "evolution-setup: $id must contain exactly one '$key:' field" >&2
            return 1
        fi
    done

    tier="$(field_value "$file" tier)"
    if [ -z "$tier" ] || [ "$tier" = "TODO" ]; then
        echo "evolution-setup: $id field 'tier:' is not filled" >&2
        return 1
    fi
    case "$tier" in
        0|1|2) ;;
        *) echo "evolution-setup: $id has an invalid tier; use 0, 1, or 2" >&2; return 1 ;;
    esac

    for key in request expected checks; do
        value="$(field_value "$file" "$key")"
        if [ -z "$value" ] || [ "$value" = "TODO" ]; then
            echo "evolution-setup: $id field '$key:' is not filled" >&2
            return 1
        fi
    done
}

validate_store() {
    ok=0
    validate_fixture "$STORE_REAL/H01.md" H01 || ok=1
    validate_fixture "$STORE_REAL/H02.md" H02 || ok=1
    return "$ok"
}

# Give the file at $1 the modification time of the pending decision: the
# commit that last changed the `Held out:` line when HEAD already says
# pending, otherwise the working copy's own time (the edit is not committed
# yet, or this is not a git checkout). A fixture older than that moment was
# written before the set was declared exposed and is stale.
pending_reference() {
    ref="$1"
    stamp=""
    if command -v git >/dev/null 2>&1 &&
        git -C "$REPO" show HEAD:docs/evolution/evaluators.md 2>/dev/null |
            grep -q '^Held out:[ \t]*pending[ \t]*$'; then
        stamp="$(git -C "$REPO" log -1 --format=%cd --date=format-local:%Y%m%d%H%M.%S \
            -S'Held out: pending' -- docs/evolution/evaluators.md 2>/dev/null || true)"
    fi
    if [ -n "$stamp" ] && touch -t "$stamp" "$ref" 2>/dev/null; then
        return 0
    fi
    touch -r "$EVAL" "$ref"
}

# Print the ids of fixture files older than the pending decision. Only
# meaningful while `Held out:` is pending; prints nothing otherwise.
stale_fixtures() {
    [ "$(held_out_value)" = "pending" ] || return 0
    ref="$(mktemp)"
    pending_reference "$ref"
    for id in H01 H02; do
        file="$STORE_REAL/$id.md"
        if [ -f "$file" ] && [ "$file" -ot "$ref" ]; then
            printf '%s\n' "$id"
        fi
    done
    rm -f "$ref"
}

prepare_fixture() {
    id="$1"
    file="$STORE_REAL/$id.md"
    [ ! -e "$file" ] || return 0

    old_umask="$(umask)"
    umask 077
    cat > "$file" <<EOF
# Private Held-Out Fixture $id

# Keep this file outside the repository and do not show it to the evolving
# agent. Replace every TODO value; keep each value on one line.
tier: TODO
request: TODO
expected: TODO
checks: TODO
EOF
    umask "$old_umask"
    chmod 600 "$file"
}

repository_gate_state() {
    baseline="$(frontmatter_value "$EVAL" baseline)"
    held_out="$(held_out_value)"
    loop="$(frontmatter_value "$CONST" loop)"
    baseline_state="not recorded"
    archive_has_baseline "$baseline" && baseline_state="recorded"
    stop_state="absent"
    [ -e "$STOP" ] && stop_state="present"

    echo "Evolution setup"
    echo "  helper ids:  H01, H02 (assigned automatically)"
    echo "  baseline:    ${baseline:-unset} ($baseline_state)"
    echo "  held out:    ${held_out:-unset}"
    echo "  loop:        ${loop:-unset}"
    echo "  STOP:        $stop_state"
}

case "$COMMAND" in
status)
    repository_gate_state
    store_ready=0
    store_stale=""
    if [ -n "$STORE" ]; then
        resolve_store
        if validate_store; then
            store_stale="$(stale_fixtures | tr '\n' ' ' | sed 's/ $//')"
            if [ -n "$store_stale" ]; then
                echo "  private set: stale - $store_stale predate the pending decision (contents not displayed)"
            else
                store_ready=1
                echo "  private set: ready (contents not displayed)"
            fi
        else
            echo "  private set: not ready"
        fi
    fi

    baseline="$(frontmatter_value "$EVAL" baseline)"
    held_out="$(held_out_value)"
    loop="$(frontmatter_value "$CONST" loop)"
    if ! archive_has_baseline "$baseline"; then
        echo "Next: record a baseline row, then set baseline: in evaluators.md."
        exit 1
    fi
    if [ -e "$STOP" ]; then
        echo "Next: STOP is engaged; only the maintainer may remove it."
        exit 1
    fi
    if [ -n "$held_out" ] && [ "$held_out" != "pending" ] && [ "$loop" = "on" ]; then
        if [ -n "$STORE" ] && [ "$store_ready" -ne 1 ]; then
            echo "Next: finish the private H01/H02 fields, then run activate."
            exit 1
        fi
        echo "Ready: repository evolution gates are active."
        exit 0
    fi
    if [ -z "$STORE" ]; then
        echo "Next: docs/scripts/evolution-setup.sh prepare --store /private/path"
    elif [ -n "$store_stale" ]; then
        echo "Next: docs/scripts/evolution-setup.sh prepare --store \"$STORE_REAL\" (retires the stale fixtures, then write fresh text)"
    elif [ "$store_ready" -eq 1 ]; then
        echo "Next: docs/scripts/evolution-setup.sh activate --store \"$STORE_REAL\""
    else
        echo "Next: finish the private H01/H02 fields, then run activate."
    fi
    exit 1
    ;;

prepare)
    resolve_store
    for id in H01 H02; do
        file="$STORE_REAL/$id.md"
        if [ -e "$file" ] && { [ ! -f "$file" ] || [ -L "$file" ]; }; then
            fail "$id.md already exists but is not a regular file" 2
        fi
    done
    # A pending set means the current fixtures are exposed: move the stale
    # ones aside (never delete them) so the templates below are fresh.
    for id in $(stale_fixtures); do
        retired="$STORE_REAL/$id.retired-$(date -u +%Y%m%dT%H%M%SZ).md"
        [ ! -e "$retired" ] || fail "$retired already exists; try again in a second" 2
        mv "$STORE_REAL/$id.md" "$retired"
        chmod 600 "$retired"
        echo "Retired stale $id.md as $(basename "$retired") (it predates the pending decision)."
    done
    prepare_fixture H01
    prepare_fixture H02
    echo "Prepared private fixtures H01 and H02 at $STORE_REAL."
    echo "Fill tier, request, expected, and checks in both files."
    echo "Then run: docs/scripts/evolution-setup.sh activate --store \"$STORE_REAL\""
    ;;

activate)
    resolve_store
    [ -f "$CONST" ] && [ ! -L "$CONST" ] || fail "docs/evolution/constitution.md must be a regular file"
    [ -f "$EVAL" ] && [ ! -L "$EVAL" ] || fail "docs/evolution/evaluators.md must be a regular file"
    [ -f "$ARCH" ] || fail "missing docs/evolution/archive.md"
    [ ! -e "$STOP" ] || fail "STOP is present; only the maintainer may remove it"
    [ -w "$CONST" ] || fail "docs/evolution/constitution.md is not writable"
    [ -w "$EVAL" ] || fail "docs/evolution/evaluators.md is not writable"

    baseline="$(frontmatter_value "$EVAL" baseline)"
    archive_has_baseline "$baseline" || fail "baseline '${baseline:-unset}' is not recorded in archive.md"
    [ "$(line_count "$CONST" "loop:")" -eq 1 ] || fail "constitution.md must contain exactly one 'loop:' line"
    [ "$(line_count "$EVAL" "Held out:")" -eq 1 ] || fail "evaluators.md must contain exactly one 'Held out:' line"
    held_out="$(held_out_value)"
    case "$held_out" in
        ""|pending|"H01, H02") ;;
        *) fail "evaluators.md already has a non-pending held-out set; no changes made" ;;
    esac
    validate_store || fail "private fixtures are not ready; no repository files changed"
    stale="$(stale_fixtures | tr '\n' ' ' | sed 's/ $//')"
    [ -z "$stale" ] || fail "$stale predate the pending decision; run prepare and write fresh text; no repository files changed"

    tmp_eval="$(mktemp)"
    tmp_const="$(mktemp)"
    backup_eval="$(mktemp)"
    backup_const="$(mktemp)"
    cleanup() { rm -f "$tmp_eval" "$tmp_const" "$backup_eval" "$backup_const"; }
    writing=0
    restore_gates() {
        restored=0
        cat "$backup_eval" > "$EVAL" || restored=1
        cat "$backup_const" > "$CONST" || restored=1
        return "$restored"
    }
    on_exit() {
        rc=$?
        trap - EXIT
        if [ "$writing" -eq 1 ]; then
            if restore_gates; then
                echo "evolution-setup: original gate content restored" >&2
            else
                echo "evolution-setup: failed to restore both gate files" >&2
                rc=1
            fi
        fi
        cleanup
        exit "$rc"
    }
    trap on_exit EXIT
    trap 'exit 1' HUP INT TERM

    awk 'index($0, "Held out:") == 1 { print "Held out: H01, H02"; next } { print }' "$EVAL" > "$tmp_eval"
    awk 'index($0, "loop:") == 1 { print "loop: on"; next } { print }' "$CONST" > "$tmp_const"
    cp "$EVAL" "$backup_eval"
    cp "$CONST" "$backup_const"

    writing=1
    cat "$tmp_eval" > "$EVAL" || fail "could not update evaluators.md"
    cat "$tmp_const" > "$CONST" || fail "could not update constitution.md"
    if [ "$(held_out_value)" != "H01, H02" ] || [ "$(frontmatter_value "$CONST" loop)" != "on" ]; then
        fail "gate verification failed"
    fi
    writing=0

    echo "Activated held-out fixtures H01 and H02 and set loop: on."
    echo "Fixture contents were not displayed or copied into the repository."
    echo "Review: git diff -- docs/evolution/evaluators.md docs/evolution/constitution.md"
    echo "The maintainer must commit the reviewed gate changes before evolution runs."
    ;;
esac
