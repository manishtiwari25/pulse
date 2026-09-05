#!/usr/bin/env bash
# The commons: the board shared by every PULSE project on this machine.
#
# A thin wrapper. It finds the commons (PULSE_COMMONS, then the registry in
# $PULSE_HOME/projects.json, then $PULSE_HOME/commons, with PULSE_HOME
# defaulting to ~/.pulse) and hands every other argument to board.sh, so the
# bounded reads agents already know (--area, --claims, --threads, --queue,
# --stale, --hash) work unchanged on the shared board. Decided in ADR-016;
# the rules are in docs/workflows/commons.md.
#
# Usage: docs/scripts/commons.sh --init [PATH]      create the commons (idempotent)
#        docs/scripts/commons.sh --path             print where the commons is
#        docs/scripts/commons.sh [board.sh options] read it, e.g. --area questions
set -eu

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOME_DIR="${PULSE_HOME:-$HOME/.pulse}"

usage() {
    cat <<'EOF'
Usage: docs/scripts/commons.sh --init [PATH]
       docs/scripts/commons.sh --path
       docs/scripts/commons.sh [--area NAME] [--claims|--threads|--queue|--stale] [--now ISO-UTC] ...

The board shared by every PULSE project on this machine (ADR-016). Agents ask
questions there that their own project cannot answer, answer others from
evidence, and record pitfalls and patterns that fit no single project.

  --init [PATH]   Create the commons as its own Git repository (default location:
                  $PULSE_COMMONS, else $PULSE_HOME/commons, else ~/.pulse/commons)
                  and remember PATH in the registry. Safe to rerun.
  --path          Print the commons directory and exit (non-zero if not created)
  anything else   Passed to board.sh --repo <commons>: --area, --claims, --threads,
                  --queue, --stale, --now, --records, --json, --hash POST, -h

Post by writing one immutable file under <commons>/docs/board/<area>/<thread>/
with the full provenance block plus `project:` and `name:`. Areas: questions,
pitfalls, patterns, runners. Never put secrets, hostnames, or private data there.
EOF
}

resolve_commons() {
    if [ -n "${PULSE_COMMONS:-}" ]; then
        printf '%s\n' "$PULSE_COMMONS"
        return
    fi
    local registry="$HOME_DIR/projects.json" from_registry=""
    if [ -f "$registry" ] && command -v python3 >/dev/null 2>&1; then
        from_registry="$(python3 - "$registry" <<'PY' 2>/dev/null || true
import json, sys
try:
    data = json.load(open(sys.argv[1], encoding="utf-8"))
    print(data.get("commons") or "")
except Exception:
    pass
PY
)"
    fi
    if [ -n "$from_registry" ]; then
        printf '%s\n' "$from_registry"
    else
        printf '%s\n' "$HOME_DIR/commons"
    fi
}

case "${1:-}" in
    -h|--help)
        usage
        exit 0
        ;;
    --init)
        shift
        if ! command -v python3 >/dev/null 2>&1; then
            echo "commons.sh: --init needs python3 (it runs pulse-dashboard.py commons-init)" >&2
            exit 1
        fi
        if [ $# -gt 0 ]; then
            exec python3 "$HERE/pulse-dashboard.py" commons-init --path "$1"
        fi
        exec python3 "$HERE/pulse-dashboard.py" commons-init
        ;;
    --path)
        target="$(resolve_commons)"
        printf '%s\n' "$target"
        [ -d "$target/docs/board" ]
        exit $?
        ;;
esac

target="$(resolve_commons)"
if [ ! -d "$target/docs/board" ]; then
    echo "commons.sh: no commons at $target (create it once with: docs/scripts/commons.sh --init)" >&2
    exit 2
fi
machine_mode=0
section="all"
area=""
ORIG_ARGS=("$@")
while [ $# -gt 0 ]; do
    case "$1" in
        --records|--json|--hash|--tsv) machine_mode=1 ;;
        --queue) section="queue" ;;
        --area) [ $# -ge 2 ] && area="$2"; shift ;;
    esac
    shift || break
done
if [ "$machine_mode" -eq 0 ] && { [ "$section" = "all" ] || [ "$section" = "queue" ]; } \
    && [ -f "${PULSE_HOME:-$HOME/.pulse}/pulse.sqlite" ]; then
    if [ "$section" = "queue" ]; then
        echo "note: a pulse store exists on this machine; if the commons is registered, prefer \`python3 docs/scripts/pulse.py queue --project commons\`"
    elif [ -n "$area" ]; then
        echo "note: a pulse store exists on this machine; if the commons is registered, prefer \`python3 docs/scripts/pulse.py board --project commons --area $area\`"
    else
        echo "note: a pulse store exists on this machine; if the commons is registered, prefer \`python3 docs/scripts/pulse.py board --project commons\`"
    fi
    echo ""
fi
exec env PULSE_SUPPRESS_STORE_NOTE=1 bash "$HERE/board.sh" --repo "$target" "${ORIG_ARGS[@]}"
