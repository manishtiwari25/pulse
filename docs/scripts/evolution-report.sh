#!/usr/bin/env bash
# Read-only report on a repository's self-improvement loop (docs/evolution/).
#
# Reads docs/evolution/constitution.md, evaluators.md, and archive.md and
# reports the kill switch, the loop flag, the baseline, counts of kept and
# discarded variants, whether voted suggestions do better than signal-derived
# hypotheses, incidents, the autonomy stage per file class, the product scope
# (whether the loop may improve the product the repository ships), hygiene
# (stray evolve/* branches and worktrees the loop must remove), and the last
# few archive rows. It never writes to the repository.
#
# Usage: docs/scripts/evolution-report.sh [--repo PATH] [--last N]
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/evolution-report.sh [--repo PATH] [--last N]

Read-only summary of docs/evolution/: kill switch, loop flag, baseline,
variant counts by decision, votes versus outcomes, incidents, autonomy stage
per file class, product scope, hygiene, and the most recent archive rows.

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
LIMITS="$(frontmatter_value "$CONST" limits | tr '[:upper:]' '[:lower:]')"
BASELINE="$(frontmatter_value "$EVAL" baseline)"
echo "Loop flag (constitution): ${LOOP:-unset}"
case "$LIMITS" in
    none) echo "Limits (constitution):    none - no budget limit binds (tokens, wall time, cycles, plateau); only stop rules end the run" ;;
    "")   [ -f "$CONST" ] && echo "Limits (constitution):    budget (limits: unset) - the Budget table binds; write limits: none to remove every limit" ;;
    *)    echo "Limits (constitution):    $LIMITS - the Budget table binds; a row written as unlimited has no limit" ;;
esac
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

# --- Product scope -----------------------------------------------------------
echo "PRODUCT SCOPE (the loop's subject is the product the repository ships)"
CLASSES_OK=1; EVALS_OK=1
if [ -f "$CONST" ]; then
    if awk -F'|' '
        function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
        /^## / { scope = ($0 ~ /^## Editable scope/); immut = ($0 ~ /^## Immutable paths/); next }
        scope && /^\|/ {
            c = trim($2); if (c == "File class" || c ~ /^-+$/) next
            st = trim($4)
            if (st == "4") { ed = ed sep c; sep = ", "; n4++ }
            else if (tolower(c) ~ /product/) { low = low sep2 c " (stage " st ")"; sep2 = ", "; nlow++ }
        }
        immut && /^\|/ {
            p = $2
            if (match(p, /`[^`]+`/)) {
                tok = substr(p, RSTART + 1, RLENGTH - 2)
                if (tok ~ /^(src|apps?|packages|services|lib|web|api|server|client|backend|frontend)\/?$/) { trees = trees sep3 "`" tok "`"; sep3 = ", "; ntrees++ }
            }
        }
        END {
            if (n4 > 0) printf "  product classes editable (stage 4): %s\n", ed
            else if (nlow == 0) print "  no product class listed - in a repository that ships a product, list its paths at stage 4 (docs/evolution/README.md, The product first)"
            if (nlow > 0) printf "  product classes listed below stage 4: %s - raise them to 4 in the constitution to let the loop improve the product\n", low
            if (ntrees > 0) printf "  immutable paths cover a whole product tree: %s - keep only hard-stop surfaces immutable; as written the loop cannot touch the product\n", trees
            exit (n4 > 0 && ntrees == 0) ? 0 : 1
        }' "$CONST"; then CLASSES_OK=0; fi
else
    echo "  no constitution"
fi
if [ -f "$EVAL" ] && grep -qiE '^##+[ \t]+Product evaluators' "$EVAL"; then
    echo "  product evaluators: present"
    EVALS_OK=0
else
    echo "  product evaluators: absent - add a '## Product evaluators' section to evaluators.md (the project's verification as a constraint, at least one product score) and a baseline row"
fi
if [ "$CLASSES_OK" -eq 0 ] && [ "$EVALS_OK" -eq 0 ]; then
    echo "  verdict: the loop can improve the product; a product hypothesis goes before an operating-layer one"
else
    echo "  verdict: the loop is confined to the operating layer (docs) until the lines above are fixed"
fi
echo ""

# --- Hygiene -----------------------------------------------------------------
echo "HYGIENE (branches, worktrees, and clones the loop must remove - docs/workflows/evolve.md, Clean)"
if git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    EXTRA="$(git -C "$REPO" worktree list --porcelain 2>/dev/null | awk '/^worktree / { n++; if (n > 1) print "    " substr($0, 10) }')"
    if [ -n "$EXTRA" ]; then
        echo "  worktrees: extra beyond the main tree - remove each when its cycle has ended:"
        printf '%s\n' "$EXTRA"
    else
        echo "  worktrees: main tree only"
    fi
    BR="$(git -C "$REPO" branch --list 'evolve/*' --format='%(refname:short)' 2>/dev/null)"
    if [ -n "$BR" ]; then
        echo "  evolve/* branches: $(printf '%s\n' "$BR" | wc -l | tr -d ' ') - delete each when its cycle ended or its pull request closed:"
        printf '%s\n' "$BR" | sed 's/^/    /'
    else
        echo "  evolve/* branches: none"
    fi
    RBR="$(git -C "$REPO" branch -r --list '*/evolve/*' --format='%(refname:short)' 2>/dev/null)"
    if [ -n "$RBR" ]; then
        echo "  remote evolve/* branches: $(printf '%s\n' "$RBR" | wc -l | tr -d ' ') - delete on the remote once the pull request merged or closed:"
        printf '%s\n' "$RBR" | sed 's/^/    /'
    fi
else
    echo "  not a git repository"
fi
echo ""

echo "Rules: docs/workflows/evolve.md. Board queue: docs/scripts/board.sh --queue."
