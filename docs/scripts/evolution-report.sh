#!/usr/bin/env bash
# Read-only report on a repository's self-improvement loop (docs/evolution/).
#
# Reads docs/evolution/constitution.md, evaluators.md, and archive.md and
# reports the kill switch, the loop flag, the baseline, counts of kept and
# discarded variants, whether voted suggestions do better than signal-derived
# hypotheses, incidents, the autonomy stage per file class, and the last few
# archive rows. It never writes to the repository.
#
# Usage: docs/scripts/evolution-report.sh [--repo PATH] [--last N]
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/evolution-report.sh [--repo PATH] [--last N]

Read-only summary of docs/evolution/: kill switch, loop flag, baseline,
variant counts by decision, votes versus outcomes, incidents, autonomy stage
per file class, and the most recent archive rows.

  --repo PATH   Repository root (default: the repository containing this script)
  --last N      Number of recent variant rows to show (default: 5)
  -h, --help    Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
LAST=5

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        --last) [ $# -ge 2 ] || { usage >&2; exit 2; }; LAST="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

EVO="$REPO/docs/evolution"
CONST="$EVO/constitution.md"
EVAL="$EVO/evaluators.md"
ARCH="$EVO/archive.md"

if [ ! -d "$EVO" ]; then
    echo "No docs/evolution/ in $REPO. See docs/evolution/README.md."
    exit 0
fi

frontmatter_value() {
    # frontmatter_value FILE KEY -> value or empty
    [ -f "$1" ] || return 0
    awk -v key="$2" '
        NR == 1 && !/^---/ { exit }
        /^---/ { c++; if (c == 2) exit; next }
        c == 1 && index($0, key ":") == 1 { v = substr($0, length(key) + 2); gsub(/^[ \t]+|[ \t]+$/, "", v); print v; exit }
    ' "$1"
}

echo "SELF-IMPROVEMENT LOOP - $REPO"
echo ""

if [ -e "$EVO/STOP" ]; then
    echo "KILL SWITCH: docs/evolution/STOP is present. Every cycle halts. Only a human removes it."
else
    echo "Kill switch: not engaged"
fi

for f in constitution evaluators archive; do
    [ -f "$EVO/$f.md" ] || echo "Missing: docs/evolution/$f.md (copy _template-$f.md; Stage 1 is not complete)"
done

LOOP="$(frontmatter_value "$CONST" loop)"
BASELINE="$(frontmatter_value "$EVAL" baseline)"
echo "Loop flag (constitution): ${LOOP:-unset}"
echo "Baseline (evaluators):    ${BASELINE:-unset}"
echo ""

if [ -f "$ARCH" ]; then
    awk -F'|' -v last="$LAST" '
    function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
    function trunc(s, n) { return length(s) > n ? substr(s, 1, n - 1) "~" : s }
    /^\| B-[0-9]+/ { baselines++ }
    /^\| I-[0-9]+/ { incidents++ }
    /^\| A-[0-9]+/ {
        n++
        row = trim($2); date = trim($3); sugg = trim($5); class = trim($7); target = trim($10); decision = trim($12)
        total++; by[decision]++
        if (sugg == "-" || sugg == "") { signal++; if (decision == "kept" || decision == "promoted") signal_kept++ }
        else { voted++; if (decision == "kept" || decision == "promoted") voted_kept++ }
        recent[n] = sprintf("  %-6s %-10s %-22s %-10s %s", row, date, trunc(class, 22), decision, trunc(target, 40))
    }
    END {
        printf "Baseline runs recorded: %d\n", baselines
        printf "Variants tried: %d", total
        if (total > 0) {
            sep = " ("
            for (d in by) { printf "%s%s %d", sep, d, by[d]; sep = ", " }
            printf ")"
        }
        print ""
        if (voted > 0) printf "  from voted suggestions: %d tried, %d kept (%.0f%%)\n", voted, voted_kept, 100 * voted_kept / voted
        if (signal > 0) printf "  from signals:           %d tried, %d kept (%.0f%%)\n", signal, signal_kept, 100 * signal_kept / signal
        if (voted > 0 && signal > 0) print "  compare the two kept rates before adjusting vote weight in the constitution"
        printf "Incidents: %d\n", incidents
        if (n > 0) {
            print ""
            printf "LAST %d VARIANTS (row, date, file class, decision, target)\n", (n < last ? n : last)
            for (i = (n - last + 1 > 1 ? n - last + 1 : 1); i <= n; i++) print recent[i]
        }
    }
    ' "$ARCH"
    echo ""
fi

if [ -f "$CONST" ]; then
    echo "EDITABLE SCOPE (file class, stage, autonomy, clean cycles so far / required, incidents)"
    awk -F'|' '
    function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
    /^## / { in_scope = ($0 ~ /^## Editable scope/) ; next }
    in_scope && /^\|/ {
        c1 = trim($2)
        if (c1 == "File class" || c1 ~ /^-+$/) next
        printf "  %-30s stage %-2s %-9s %s/%s  incidents %s\n", c1, trim($4), trim($5), trim($7), trim($6), trim($8)
        rows++
    }
    END { if (rows == 0) print "  none (the constitution has no editable-scope rows yet)" }
    ' "$CONST"
    echo ""
fi

echo "Rules: docs/workflows/evolve.md. Board queue: docs/scripts/board.sh --queue."
