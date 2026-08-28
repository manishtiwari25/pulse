#!/usr/bin/env bash
# Optional, best-effort token diagnostics across AI coding runners. Each
# collector reads a local runner store and reports only token fields it finds.
# Local formats can change, so this is not a task-completion check, ledger
# writer, billing source, or authoritative telemetry system.
#
#   claude    Claude Code (CLI, IDE extensions, desktop)   ~/.claude/projects
#   copilot   GitHub Copilot CLI                           ~/.copilot/session-state
#   opencode  OpenCode (TUI and IDE)                       ~/.local/share/opencode
#   codex     OpenAI Codex (CLI and IDE extension)         ~/.codex/sessions
#
# Usage: docs/scripts/usage.sh [all|claude|copilot|opencode|codex] [session-id]
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
PICK="${1:-all}"
SID="${2:-}"

case "$PICK" in
    -h|--help)
        echo "Usage: docs/scripts/usage.sh [all|claude|copilot|opencode|codex] [session-id]"
        echo "Optional diagnostics only; local runner formats may change."
        exit 0
        ;;
    vscode|cursor)
        echo "$PICK does not expose token counts to this diagnostic; no collector is provided." >&2
        exit 2
        ;;
    all|claude|copilot|opencode|codex) ;;
    *)
        SID="$PICK"
        PICK="all"
        ;;
esac

ran=0

run() {
    # $1 name, $2 data path, $3 collector
    [ "$PICK" = "all" ] || [ "$PICK" = "$1" ] || return 0
    if [ -e "$2" ]; then
        echo "== $1 =="
        "$HERE/$3" "$SID" || true
        ran=1
    else
        if [ "$PICK" = "$1" ]; then
            echo "$1: not detected ($2 missing)"
            ran=1
        else
            echo "== $1 == not detected"
        fi
    fi
    echo
}

CLAUDE_ROOT="${CLAUDE_CONFIG_DIR:-$HOME/.claude}"
[ -d "$CLAUDE_ROOT" ] || CLAUDE_ROOT="$HOME/.config/claude"
run claude   "$CLAUDE_ROOT"                                                     usage-claude.sh
run copilot  "${COPILOT_STATE_DIR:-$HOME/.copilot/session-state}"             usage-copilot.sh
run opencode "${OPENCODE_STORE:-$HOME/.local/share/opencode/storage/message}" usage-opencode.sh
run codex    "${CODEX_HOME:-$HOME/.codex}/sessions"                           usage-codex.sh

[ "$ran" = 1 ] || echo "No supported AI-runner logs detected on this machine."
