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
# Usage: docs/scripts/evolution-report.sh [--repo PATH] [--last N] [--records|--json]
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/evolution-report.sh [--repo PATH] [--last N] [--records|--json]

Read-only summary of docs/evolution/: kill switch, loop flag, baseline,
variant counts by decision, votes versus outcomes, incidents, autonomy stage
per file class, product scope, hygiene, and the most recent archive rows.

  --repo PATH   Repository root (default: the repository containing this script)
  --last N      Number of recent variant rows to show (default: 5)
  --records     Emit typed tab-separated records (param, archive_header, archive_row,
                archive_count, decision, origin, scope, product, hygiene) computed by
                the same code as the text report; for local tools such as the dashboard
  --json        Emit a versioned JSON snapshot for local tools (requires Python 3)
  -h, --help    Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
LAST=5
RECORDS=0
JSON=0

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        --last) [ $# -ge 2 ] || { usage >&2; exit 2; }; LAST="$2"; shift 2 ;;
        --records) RECORDS=1; shift ;;
        --json) JSON=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

if [ "$JSON" -eq 1 ]; then
    [ "$RECORDS" -eq 0 ] || { echo "--json cannot be combined with --records" >&2; exit 2; }
    command -v python3 >/dev/null 2>&1 || { echo "--json requires python3" >&2; exit 1; }
    [ -f "$HERE/pulse-dashboard.py" ] || { echo "Missing docs/scripts/pulse-dashboard.py" >&2; exit 1; }
    exec python3 "$HERE/pulse-dashboard.py" _evolution-snapshot --repo "$REPO" --last "$LAST"
fi

# Text lines go to stdout as before; in records mode the same values are
# printed as typed records through these two helpers.
say() { [ "$RECORDS" -eq 0 ] && printf '%s\n' "$1"; return 0; }
rec() { [ "$RECORDS" -eq 1 ] && { local IFS; IFS="$(printf '\t')"; printf '%s\n' "$*"; }; return 0; }

EVO="$REPO/docs/evolution"
CONST="$EVO/constitution.md"
EVAL="$EVO/evaluators.md"
ARCH="$EVO/archive.md"

if [ ! -d "$EVO" ]; then
    rec param evolution_present 0
    say "No docs/evolution/ in $REPO. See docs/evolution/README.md."
    exit 0
fi
rec param evolution_present 1
rec param repo "$REPO"

frontmatter_value() {
    # frontmatter_value FILE KEY -> value or empty
    [ -f "$1" ] || return 0
    awk -v key="$2" '
        NR == 1 && !/^---/ { exit }
        /^---/ { c++; if (c == 2) exit; next }
        c == 1 && index($0, key ":") == 1 { v = substr($0, length(key) + 2); gsub(/^[ \t]+|[ \t]+$/, "", v); print v; exit }
    ' "$1"
}

say "SELF-IMPROVEMENT LOOP - $REPO"
say ""

if [ -e "$EVO/STOP" ]; then
    rec param stop_present 1
    say "KILL SWITCH: docs/evolution/STOP is present. Every cycle halts. Only a human removes it."
else
    rec param stop_present 0
    say "Kill switch: not engaged"
fi

for f in constitution evaluators archive; do
    if [ ! -f "$EVO/$f.md" ]; then
        rec param missing "$f"
        say "Missing: docs/evolution/$f.md (copy _template-$f.md; Stage 1 is not complete)"
    fi
done

LOOP="$(frontmatter_value "$CONST" loop)"
LIMITS="$(frontmatter_value "$CONST" limits | tr '[:upper:]' '[:lower:]')"
BASELINE="$(frontmatter_value "$EVAL" baseline)"
rec param loop "$LOOP"
rec param limits "$LIMITS"
rec param baseline "$BASELINE"
say "Loop flag (constitution): ${LOOP:-unset}"
LIMITS_NOTE=""
case "$LIMITS" in
    none) LIMITS_NOTE="none - no budget limit binds (tokens, wall time, cycles, plateau); only stop rules end the run" ;;
    "")   [ -f "$CONST" ] && LIMITS_NOTE="budget (limits: unset) - the Budget table binds; write limits: none to remove every limit" ;;
    *)    LIMITS_NOTE="$LIMITS - the Budget table binds; a row written as unlimited has no limit" ;;
esac
if [ -n "$LIMITS_NOTE" ]; then
    rec param limits_note "$LIMITS_NOTE"
    say "Limits (constitution):    $LIMITS_NOTE"
fi
say "Baseline (evaluators):    ${BASELINE:-unset}"
say ""

if [ -f "$ARCH" ]; then
    awk -F'|' -v last="$LAST" -v records="$RECORDS" '
    function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
    function trunc(s, n) { return length(s) > n ? substr(s, 1, n - 1) "~" : s }
    function cells(prefix,    i, out) {
        out = prefix
        for (i = 2; i < NF; i++) out = out "\t" trim($i)
        return out
    }
    /^## / { section = trim(substr($0, 4)); header_done = 0; next }
    records && /^\|/ && !header_done && trim($2) !~ /^-+$/ { header_done = 1; print cells("archive_header\t" section); next }
    /^\| B-[0-9]+/ { baselines++; if (records) print cells("archive_row\t" section) }
    /^\| I-[0-9]+/ { incidents++; if (records) print cells("archive_row\t" section) }
    /^\| A-[0-9]+/ {
        n++
        row = trim($2); date = trim($3); sugg = trim($5); class = trim($7); target = trim($10); decision = trim($12)
        total++; by[decision]++
        if (sugg == "-" || sugg == "") { signal++; if (decision == "kept" || decision == "promoted") signal_kept++ }
        else { voted++; if (decision == "kept" || decision == "promoted") voted_kept++ }
        recent[n] = sprintf("  %-6s %-10s %-22s %-10s %s", row, date, trunc(class, 22), decision, trunc(target, 40))
        if (records) print cells("archive_row\t" section)
    }
    END {
        if (records) {
            printf "archive_count\tbaselines\t%d\n", baselines
            printf "archive_count\tvariants\t%d\n", total
            printf "archive_count\tincidents\t%d\n", incidents
            for (d in by) printf "decision\t%s\t%d\n", d, by[d]
            printf "origin\tvoted\t%d\t%d\n", voted, voted_kept
            printf "origin\tsignal\t%d\t%d\n", signal, signal_kept
            exit
        }
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
    say ""
fi

if [ -f "$CONST" ]; then
    say "EDITABLE SCOPE (file class, stage, autonomy, clean cycles so far / required, incidents)"
    awk -F'|' -v records="$RECORDS" '
    function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
    /^## / { in_scope = ($0 ~ /^## Editable scope/) ; next }
    in_scope && /^\|/ {
        c1 = trim($2)
        if (c1 == "File class" || c1 ~ /^-+$/) next
        if (records) printf "scope\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", c1, trim($3), trim($4), trim($5), trim($6), trim($7), trim($8)
        else printf "  %-30s stage %-2s %-9s %s/%s  incidents %s\n", c1, trim($4), trim($5), trim($7), trim($6), trim($8)
        rows++
    }
    END { if (rows == 0 && !records) print "  none (the constitution has no editable-scope rows yet)" }
    ' "$CONST"
    say ""
fi

# --- Product scope -----------------------------------------------------------
say "PRODUCT SCOPE (the loop's subject is the product the repository ships)"
CLASSES_OK=1; EVALS_OK=1
if [ -f "$CONST" ]; then
    if awk -F'|' -v records="$RECORDS" '
        function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
        function note(s) { if (records) print "product\tnote\t" s; else print "  " s }
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
            if (n4 > 0) note("product classes editable (stage 4): " ed)
            else if (nlow == 0) note("no product class listed - in a repository that ships a product, list its paths at stage 4 (docs/evolution/README.md, The product first)")
            if (nlow > 0) note("product classes listed below stage 4: " low " - raise them to 4 in the constitution to let the loop improve the product")
            if (ntrees > 0) note("immutable paths cover a whole product tree: " trees " - keep only hard-stop surfaces immutable; as written the loop cannot touch the product")
            exit (n4 > 0 && ntrees == 0) ? 0 : 1
        }' "$CONST"; then CLASSES_OK=0; fi
else
    rec product note "no constitution"
    say "  no constitution"
fi
if [ -f "$EVAL" ] && grep -qiE '^##+[ \t]+Product evaluators' "$EVAL"; then
    rec product note "product evaluators: present"
    say "  product evaluators: present"
    EVALS_OK=0
else
    rec product note "product evaluators: absent - add a '## Product evaluators' section to evaluators.md (the project's verification as a constraint, at least one product score) and a baseline row"
    say "  product evaluators: absent - add a '## Product evaluators' section to evaluators.md (the project's verification as a constraint, at least one product score) and a baseline row"
fi
rec product classes_ok "$((1 - CLASSES_OK))"
rec product evals_ok "$((1 - EVALS_OK))"
if [ "$CLASSES_OK" -eq 0 ] && [ "$EVALS_OK" -eq 0 ]; then
    rec product verdict "the loop can improve the product; a product hypothesis goes before an operating-layer one"
    say "  verdict: the loop can improve the product; a product hypothesis goes before an operating-layer one"
else
    rec product verdict "the loop is confined to the operating layer (docs) until the lines above are fixed"
    say "  verdict: the loop is confined to the operating layer (docs) until the lines above are fixed"
fi
say ""

# --- Hygiene -----------------------------------------------------------------
say "HYGIENE (branches, worktrees, and clones the loop must remove - docs/workflows/evolve.md, Clean)"
if git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    rec hygiene git 1
    EXTRA="$(git -C "$REPO" worktree list --porcelain 2>/dev/null | awk '/^worktree / { n++; if (n > 1) print "    " substr($0, 10) }')"
    if [ -n "$EXTRA" ]; then
        say "  worktrees: extra beyond the main tree - remove each when its cycle has ended:"
        [ "$RECORDS" -eq 0 ] && printf '%s\n' "$EXTRA"
        [ "$RECORDS" -eq 1 ] && printf '%s\n' "$EXTRA" | sed 's/^    /hygiene'"$(printf '\t')"'worktree'"$(printf '\t')"'/'
    else
        say "  worktrees: main tree only"
    fi
    BR="$(git -C "$REPO" branch --list 'evolve/*' --format='%(refname:short)' 2>/dev/null)"
    if [ -n "$BR" ]; then
        say "  evolve/* branches: $(printf '%s\n' "$BR" | wc -l | tr -d ' ') - delete each when its cycle ended or its pull request closed:"
        [ "$RECORDS" -eq 0 ] && printf '%s\n' "$BR" | sed 's/^/    /'
        [ "$RECORDS" -eq 1 ] && printf '%s\n' "$BR" | sed 's/^/hygiene'"$(printf '\t')"'branch'"$(printf '\t')"'/'
    else
        say "  evolve/* branches: none"
    fi
    RBR="$(git -C "$REPO" branch -r --list '*/evolve/*' --format='%(refname:short)' 2>/dev/null)"
    if [ -n "$RBR" ]; then
        say "  remote evolve/* branches: $(printf '%s\n' "$RBR" | wc -l | tr -d ' ') - delete on the remote once the pull request merged or closed:"
        [ "$RECORDS" -eq 0 ] && printf '%s\n' "$RBR" | sed 's/^/    /'
        [ "$RECORDS" -eq 1 ] && printf '%s\n' "$RBR" | sed 's/^/hygiene'"$(printf '\t')"'remote_branch'"$(printf '\t')"'/'
    fi
else
    rec hygiene git 0
    say "  not a git repository"
fi
say ""

say "Rules: docs/workflows/evolve.md. Board queue: docs/scripts/board.sh --queue."
