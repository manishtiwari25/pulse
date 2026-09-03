#!/usr/bin/env bash
# Read-only index of the PULSE agents' board (docs/board/).
#
# Reads the frontmatter of every post under docs/board/<area>/<thread>/*.md
# and reports active claims, open threads, the experiment queue (suggestions
# ordered by votes), and stale claims and threads. It never writes to the
# repository. Rules live in docs/workflows/board.md and the project's
# docs/evolution/constitution.md; the defaults below match the constitution
# template and can be overridden per run.
#
# Usage: docs/scripts/board.sh [--repo PATH] [--area NAME] [--now ISO-UTC]
#                              [--quorum N] [--expiry-days N] [--stale-days N]
#                              [--claims|--threads|--queue|--stale] [--tsv]
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/board.sh [--repo PATH] [--area NAME] [--now ISO-UTC]
                             [--quorum N] [--expiry-days N] [--stale-days N]
                             [--claims|--threads|--queue|--stale] [--tsv]

Read-only index of docs/board/: active claims, open threads with their latest
summary line, the experiment queue ordered by votes, and stale items.

  --repo PATH        Repository root (default: the repository containing this script)
  --area NAME        Only this channel (default: all)
  --now ISO-UTC      Reference time, e.g. 2026-09-03T08:00:00Z (default: now)
  --quorum N         Distinct voters a suggestion needs to enter the queue (default: 2)
  --expiry-days N    Days after which an unvoted suggestion expires (default: 14)
  --stale-days N     Days without a post after which an open thread is stale (default: 30)
  --claims           Show only active claims
  --threads          Show only open threads
  --queue            Show only the experiment queue
  --stale            Show only expired claims, expired suggestions, and stale threads
  --tsv              Emit one tab-separated row per post and exit
  -h, --help         Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
AREA=""
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
QUORUM=2
EXPIRY_DAYS=14
STALE_DAYS=30
SECTION=all
TSV=0

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        --area) [ $# -ge 2 ] || { usage >&2; exit 2; }; AREA="$2"; shift 2 ;;
        --now) [ $# -ge 2 ] || { usage >&2; exit 2; }; NOW="$2"; shift 2 ;;
        --quorum) [ $# -ge 2 ] || { usage >&2; exit 2; }; QUORUM="$2"; shift 2 ;;
        --expiry-days) [ $# -ge 2 ] || { usage >&2; exit 2; }; EXPIRY_DAYS="$2"; shift 2 ;;
        --stale-days) [ $# -ge 2 ] || { usage >&2; exit 2; }; STALE_DAYS="$2"; shift 2 ;;
        --claims|--threads|--queue|--stale) SECTION="${1#--}"; shift ;;
        --tsv) TSV=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

BOARD="$REPO/docs/board"
if [ ! -d "$BOARD" ]; then
    echo "No docs/board/ in $REPO. See docs/board/README.md."
    exit 0
fi

LIST="$(mktemp)"
ROWS="$(mktemp)"
QUEUE="$(mktemp)"
trap 'rm -f "$LIST" "$ROWS" "$QUEUE"' EXIT

find "$BOARD" -mindepth 3 -maxdepth 3 -type f -name '*.md' | LC_ALL=C sort > "$LIST"
if [ ! -s "$LIST" ]; then
    echo "Board is empty (no posts under docs/board/<area>/<thread>/)."
    exit 0
fi

# One tab-separated row per post:
# path area thread id kind agent date expires on vote resolution summary
tr '\n' '\0' < "$LIST" | xargs -0 awk '
function emit() {
    gsub(/\t/, " ", v["summary"])
    printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", FILENAME, area, thread, id, \
        v["kind"], v["agent"], v["date"], v["expires"], v["on"], v["vote"], v["resolution"], v["summary"]
}
FNR == 1 {
    state = 0
    split("", v)
    n = split(FILENAME, parts, "/")
    id = parts[n]; sub(/\.md$/, "", id)
    thread = parts[n - 1]
    area = parts[n - 2]
}
/^---[ \t]*$/ {
    if (state == 0) { state = 1; next }
    if (state == 1) { emit(); nextfile }
}
state == 1 && match($0, /^[a-z_]+:/) {
    key = substr($0, 1, RLENGTH - 1)
    val = substr($0, RLENGTH + 1)
    gsub(/^[ \t]+|[ \t]+$/, "", val)
    if (val ~ /^".*"$/) val = substr(val, 2, length(val) - 2)
    v[key] = val
}
' | awk -F'\t' -v a="$AREA" 'a == "" || $2 == a' > "$ROWS"

if [ ! -s "$ROWS" ]; then
    echo "No posts${AREA:+ in area \"$AREA\"}."
    exit 0
fi

if [ "$TSV" -eq 1 ]; then
    cat "$ROWS"
    exit 0
fi

# Everything below is computed from the rows; the queue is sorted afterwards.
awk -F'\t' -v now="$NOW" -v quorum="$QUORUM" -v expiry_days="$EXPIRY_DAYS" \
    -v stale_days="$STALE_DAYS" -v section="$SECTION" -v queue_file="$QUEUE" '
function days_from_civil(y, m, d,    era, yoe, doy, doe) {
    if (m <= 2) y -= 1
    era = (y >= 0 ? y : y - 399) / 400; era = int(era)
    yoe = y - era * 400
    doy = int((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1
    doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
    return era * 146097 + doe - 719468
}
function day_number(s,    a) {
    split(substr(s, 1, 10), a, "-")
    return days_from_civil(a[1] + 0, a[2] + 0, a[3] + 0)
}
function trunc(s, n) { return length(s) > n ? substr(s, 1, n - 1) "~" : s }
function show(name) { return section == "all" || section == name }
{
    path = $1; area = $2; thread = $3; id = $4; kind = $5; agent = $6
    date = $7; expires = $8; on = $9; vote = $10; res = $11; summary = $12
    key = area "/" thread
    nposts[key]++
    if (!(key in last) || date >= last[key]) { last[key] = date; lastsum[key] = summary }
    if (res != "") { closed[key] = 1; resolution[key] = res }
    if (kind == "claim") {
        claim_key[id] = key; claim_agent[id] = agent; claim_exp[id] = expires; claim_sum[id] = summary
    }
    if (kind == "suggestion") {
        sugg_key[id] = key; sugg_agent[id] = agent; sugg_date[id] = date; sugg_sum[id] = summary
    }
    if (kind == "vote" && on != "") {
        vk = on SUBSEP agent
        if (!(vk in vote_date) || date >= vote_date[vk]) { vote_date[vk] = date; vote_val[vk] = vote }
    }
    if (kind == "") nokind++
}
END {
    today_n = day_number(now)

    if (show("claims")) {
        printf "ACTIVE CLAIMS (now %s)\n", now
        n = 0
        for (id in claim_key) {
            if (claim_key[id] in closed) continue
            if (claim_exp[id] != "" && claim_exp[id] > now) {
                printf "  %-38s %-24s until %-20s %s\n", claim_key[id], trunc(claim_agent[id], 24), claim_exp[id], trunc(claim_sum[id], 60); n++
            }
        }
        if (n == 0) print "  none"
        print ""
    }

    if (show("threads")) {
        print "OPEN THREADS"
        n = 0
        for (key in nposts) {
            if (key in closed) continue
            printf "  %-38s %2d posts  last %-20s %s\n", key, nposts[key], last[key], trunc(lastsum[key], 60); n++
        }
        if (n == 0) print "  none"
        print ""
    }

    # Queue rows go to a file so the shell can sort them by score, then date.
    for (id in sugg_key) {
        if (sugg_key[id] in closed) continue
        up = 0; down = 0; voters = 0
        for (vk in vote_val) {
            split(vk, p, SUBSEP)
            if (p[1] != id) continue
            if (p[2] == sugg_agent[id]) continue
            voters++
            if (vote_val[vk] == "down") down++; else up++
        }
        score = up - down
        age = today_n - day_number(sugg_date[id])
        if (voters >= quorum) status = "eligible"
        else if (voters == 0 && age > expiry_days) status = "expired"
        else status = "needs votes"
        printf "%d\t%s\t%s\t%s\t%d\t%d\t%d\t%s\t%s\n", score, sugg_date[id], id, sugg_key[id], up, down, voters, status, sugg_sum[id] > queue_file
    }

    if (show("stale")) {
        printf "STALE (expired claims, expired suggestions, threads idle > %d days)\n", stale_days
        n = 0
        for (id in claim_key) {
            if (claim_key[id] in closed) continue
            if (claim_exp[id] == "" || claim_exp[id] <= now) {
                printf "  claim      %-38s %-24s expired %s\n", claim_key[id], trunc(claim_agent[id], 24), (claim_exp[id] == "" ? "(no expiry)" : claim_exp[id]); n++
            }
        }
        for (key in nposts) {
            if (key in closed) continue
            if (today_n - day_number(last[key]) > stale_days) {
                printf "  thread     %-38s idle since %s\n", key, last[key]; n++
            }
        }
        if (nokind > 0) { printf "  %d post(s) without a kind: prune them\n", nokind; n++ }
        if (n == 0) print "  none (expired suggestions, if any, are listed in the queue)"
        print ""
    }
}
' "$ROWS"

if [ "$SECTION" = "all" ] || [ "$SECTION" = "queue" ] || [ "$SECTION" = "stale" ]; then
    echo "EXPERIMENT QUEUE (score = up - down; quorum $QUORUM distinct voters; unvoted expire after $EXPIRY_DAYS days)"
    if [ -s "$QUEUE" ]; then
        TAB="$(printf '\t')"
        sort -t "$TAB" -k1,1nr -k2,2 "$QUEUE" | awk -F'\t' -v section="$SECTION" '
        function trunc(s, n) { return length(s) > n ? substr(s, 1, n - 1) "~" : s }
        {
            if (section == "stale" && $8 != "expired") next
            printf "  %+3d  %-12s %-38s %-44s up %d down %d voters %d  %s\n", $1, $8, $4, trunc($3, 44), $5, $6, $7, trunc($9, 50)
        }'
    else
        echo "  none"
    fi
fi
