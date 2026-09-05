#!/usr/bin/env bash
# Read-only index of the PULSE agents' board (docs/board/).
#
# Reads the frontmatter of every post under docs/board/<area>/<thread>/*.md
# and reports active claims, open threads, the experiment queue (suggestions
# ordered by votes), constitutional amendments with their tallies, and stale
# claims and threads. It never writes to the repository. Rules live in
# docs/workflows/board.md, docs/workflows/amend.md, and the project's
# docs/evolution/constitution.md; the defaults below match the constitution
# template and can be overridden per run.
#
# Usage: docs/scripts/board.sh [--repo PATH] [--area NAME] [--now ISO-UTC]
#                              [--quorum N] [--expiry-days N] [--stale-days N]
#                              [--amend-quorum N] [--amend-majority P/Q]
#                              [--agent ID] [--no-notices]
#                              [--claims|--threads|--proposals|--queue|--amendments|--stale|--ballot]
#                              [--tsv|--records|--json]
#        docs/scripts/board.sh --hash POST
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/board.sh [--repo PATH] [--area NAME] [--now ISO-UTC]
                             [--quorum N] [--expiry-days N] [--stale-days N]
                             [--amend-quorum N] [--amend-majority P/Q]
                             [--agent ID] [--no-notices]
                             [--claims|--threads|--proposals|--queue|--amendments|--stale|--ballot]
                             [--tsv|--records|--json]
       docs/scripts/board.sh --hash POST

Read-only index of docs/board/: notices (who is asking for a vote here, on the
brain, and in the commons), active claims, open threads with their latest
summary line, proposals the agents agreed on that wait for the human's yes or
no, the experiment queue ordered by votes, constitutional amendments with their
tallies, stale items, and the ballot an identity can still vote on.

  --repo PATH          Repository root (default: the repository containing this script)
  --area NAME          Only this channel (default: all; amendments are always repository-wide)
  --agent ID           Your identity (<runner>/<model>; default: $PULSE_AGENT). The ballot
                       then hides what you authored or already voted on
  --no-notices         Do not print the NOTICES block at the top of the text report
  --now ISO-UTC        Reference time, e.g. 2026-09-03T08:00:00Z (default: now)
  --quorum N           Distinct voters a suggestion needs to enter the queue (default: 2)
  --expiry-days N      Days after which an unvoted suggestion expires (default: 14)
  --stale-days N       Days without a post after which an open thread is stale (default: 30)
  --amend-quorum N     Distinct voting sessions an amendment needs
                       (default: the constitution's "Amendment quorum" row, else 40)
  --amend-majority P/Q Share of counted votes that must be up
                       (default: the constitution's "Amendment supermajority" row, else 2/3)
  --claims             Show only active claims
  --threads            Show only open threads
  --proposals          Show only proposals for the human: threads whose latest answer
                       carries resolution: agreed, until a result post records the
                       human's yes (resolution: accepted) or no (resolution: declined)
  --queue              Show only the experiment queue
  --amendments         Show only constitutional amendments and their tallies
  --stale              Show only expired claims, expired suggestions, and stale threads
  --ballot             Show only what is open for a vote: suggestions and amendments,
                       minus your own and those you voted on when --agent is given.
                       With --tsv: one row per item (kind status key id up down
                       voters author text_hash summary) for other scripts
  --tsv                Emit one tab-separated row per post and exit
  --records            Emit typed tab-separated records (param, post, claim, thread,
                       proposal, suggestion, svote, amendment, avote, nokind) computed
                       by the same tally as the text report; for local tools such as
                       the dashboard. Cannot be combined with a section flag or --tsv
  --json               Emit a versioned JSON snapshot for local tools
                       (requires Python 3; cannot be combined with a section or --tsv)
  --hash POST          Print the sha256 of an amendment post's "## Proposed text" block
                       (the text_hash a vote must carry) and exit
  -h, --help           Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
AREA=""
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
QUORUM=2
EXPIRY_DAYS=14
STALE_DAYS=30
AMEND_QUORUM=""
AMEND_MAJORITY=""
HASH_POST=""
SECTION=all
SECTION_EXPLICIT=0
TSV=0
RECORDS=0
JSON=0
BALLOT_AGENT="${PULSE_AGENT:-}"
NOTICES=1

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        --area) [ $# -ge 2 ] || { usage >&2; exit 2; }; AREA="$2"; shift 2 ;;
        --now) [ $# -ge 2 ] || { usage >&2; exit 2; }; NOW="$2"; shift 2 ;;
        --quorum) [ $# -ge 2 ] || { usage >&2; exit 2; }; QUORUM="$2"; shift 2 ;;
        --expiry-days) [ $# -ge 2 ] || { usage >&2; exit 2; }; EXPIRY_DAYS="$2"; shift 2 ;;
        --stale-days) [ $# -ge 2 ] || { usage >&2; exit 2; }; STALE_DAYS="$2"; shift 2 ;;
        --amend-quorum) [ $# -ge 2 ] || { usage >&2; exit 2; }; AMEND_QUORUM="$2"; shift 2 ;;
        --amend-majority) [ $# -ge 2 ] || { usage >&2; exit 2; }; AMEND_MAJORITY="$2"; shift 2 ;;
        --hash) [ $# -ge 2 ] || { usage >&2; exit 2; }; HASH_POST="$2"; shift 2 ;;
        --agent) [ $# -ge 2 ] || { usage >&2; exit 2; }; BALLOT_AGENT="$2"; shift 2 ;;
        --no-notices) NOTICES=0; shift ;;
        --claims|--threads|--proposals|--queue|--amendments|--stale|--ballot) SECTION="${1#--}"; SECTION_EXPLICIT=1; shift ;;
        --tsv) TSV=1; shift ;;
        --records) RECORDS=1; shift ;;
        --json) JSON=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

if [ "$RECORDS" -eq 1 ]; then
    [ "$TSV" -eq 0 ] || { echo "--records cannot be combined with --tsv" >&2; exit 2; }
    [ "$JSON" -eq 0 ] || { echo "--records cannot be combined with --json" >&2; exit 2; }
    [ "$SECTION_EXPLICIT" -eq 0 ] || { echo "--records emits every section and cannot be combined with a section flag" >&2; exit 2; }
    [ -z "$HASH_POST" ] || { echo "--records cannot be combined with --hash" >&2; exit 2; }
fi

if [ "$JSON" -eq 1 ]; then
    [ "$TSV" -eq 0 ] || { echo "--json cannot be combined with --tsv" >&2; exit 2; }
    [ "$SECTION_EXPLICIT" -eq 0 ] || { echo "--json emits the complete snapshot and cannot be combined with a section flag" >&2; exit 2; }
    [ -z "$HASH_POST" ] || { echo "--json cannot be combined with --hash" >&2; exit 2; }
    command -v python3 >/dev/null 2>&1 || { echo "--json requires python3" >&2; exit 1; }
    [ -f "$HERE/pulse-dashboard.py" ] || { echo "Missing docs/scripts/pulse-dashboard.py" >&2; exit 1; }

    JSON_ARGS=(
        _board-snapshot
        --repo "$REPO"
        --now "$NOW"
        --quorum "$QUORUM"
        --expiry-days "$EXPIRY_DAYS"
        --stale-days "$STALE_DAYS"
    )
    [ -n "$AREA" ] && JSON_ARGS+=(--area "$AREA")
    [ -n "$AMEND_QUORUM" ] && JSON_ARGS+=(--amend-quorum "$AMEND_QUORUM")
    [ -n "$AMEND_MAJORITY" ] && JSON_ARGS+=(--amend-majority "$AMEND_MAJORITY")
    exec python3 "$HERE/pulse-dashboard.py" "${JSON_ARGS[@]}"
fi

if [ "${PULSE_SUPPRESS_STORE_NOTE:-0}" != "1" ] && [ "$TSV" -eq 0 ] && [ "$RECORDS" -eq 0 ] \
    && [ -z "$HASH_POST" ] && { [ "$SECTION" = "all" ] || [ "$SECTION" = "queue" ]; }; then
    STORE_FILE="${PULSE_HOME:-$HOME/.pulse}/pulse.sqlite"
    if [ -f "$STORE_FILE" ]; then
        if [ "$SECTION" = "queue" ]; then
            echo "note: a pulse store exists on this machine; if this project is registered, prefer \`python3 docs/scripts/pulse.py queue\`"
        elif [ -n "$AREA" ]; then
            echo "note: a pulse store exists on this machine; if this project is registered, prefer \`python3 docs/scripts/pulse.py board --area $AREA\`"
        else
            echo "note: a pulse store exists on this machine; if this project is registered, prefer \`python3 docs/scripts/pulse.py board\`"
        fi
        echo ""
    fi
fi

# The fenced block under "## <heading>" in an amendment post, normalized the
# way every tally and every voter must see it: trailing whitespace stripped,
# leading and trailing blank lines dropped, one newline after each line.
# Exit 1 when the heading, the fence, or the text is missing.
block() {
    awk -v heading="$2" '
    BEGIN { st = 0; n = 0 }
    st == 0 && $0 ~ ("^## " heading "[ \t]*$") { st = 1; next }
    st == 1 && /^## / { exit }
    st == 1 && /^```/ { match($0, /^`+/); fence = substr($0, 1, RLENGTH); st = 2; next }
    st == 2 && $0 ~ ("^" fence "[ \t]*$") { st = 3; exit }
    st == 2 { sub(/[ \t]+$/, ""); lines[++n] = $0 }
    END {
        if (st < 2) exit 1
        s = 1; e = n
        while (s <= e && lines[s] == "") s++
        while (e >= s && lines[e] == "") e--
        if (s > e) exit 1
        for (i = s; i <= e; i++) print lines[i]
    }' "$1"
}

sha256() {
    if command -v shasum >/dev/null 2>&1; then shasum -a 256 | awk '{ print $1 }'
    elif command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{ print $1 }'
    else openssl dgst -sha256 | awk '{ print $NF }'
    fi
}

# The value cell of "| <name> | <value> |" in a Markdown table, trimmed.
row() {
    awk -F'|' -v name="$2" '
    NF >= 3 {
        k = $2; gsub(/^[ \t]+|[ \t]+$/, "", k)
        if (k == name) { v = $3; gsub(/^[ \t]+|[ \t]+$/, "", v); print v; exit }
    }' "$1"
}

if [ -n "$HASH_POST" ]; then
    [ -f "$HASH_POST" ] || { echo "No such post: $HASH_POST" >&2; exit 2; }
    BLOCK="$(mktemp)"
    trap 'rm -f "$BLOCK"' EXIT
    if ! block "$HASH_POST" "Proposed text" > "$BLOCK"; then
        echo "No fenced block under '## Proposed text' in $HASH_POST (see docs/workflows/amend.md)." >&2
        exit 1
    fi
    sha256 < "$BLOCK"
    exit 0
fi

BOARD="$REPO/docs/board"
if [ ! -d "$BOARD" ]; then
    if [ "$RECORDS" -eq 1 ]; then printf 'param\tboard_present\t0\n'; exit 0; fi
    [ "$TSV" -eq 1 ] && exit 0
    echo "No docs/board/ in $REPO. See docs/board/README.md."
    exit 0
fi

# Amendment thresholds: flags win, then the constitution's Vote rules rows,
# then the template defaults.
CONSTITUTION="$REPO/docs/evolution/constitution.md"
PROTECTED_ROW=""
if [ -f "$CONSTITUTION" ]; then
    if [ -z "$AMEND_QUORUM" ]; then
        AMEND_QUORUM="$(row "$CONSTITUTION" "Amendment quorum" | awk '{ if (match($0, /[0-9]+/)) print substr($0, RSTART, RLENGTH) }')"
    fi
    if [ -z "$AMEND_MAJORITY" ]; then
        AMEND_MAJORITY="$(row "$CONSTITUTION" "Amendment supermajority" | awk '{ if (match($0, /[0-9]+\/[0-9]+/)) print substr($0, RSTART, RLENGTH) }')"
    fi
    PROTECTED_ROW="$(row "$CONSTITUTION" "Not amendable by vote")"
fi
[ -n "$AMEND_QUORUM" ] || AMEND_QUORUM=40
[ -n "$AMEND_MAJORITY" ] || AMEND_MAJORITY=2/3
[ -n "$PROTECTED_ROW" ] || PROTECTED_ROW="Hard stops, Kill switch, Amendments, frontmatter"
case "$AMEND_MAJORITY" in
    */*) AMEND_NUM="${AMEND_MAJORITY%%/*}"; AMEND_DEN="${AMEND_MAJORITY#*/}" ;;
    *) echo "--amend-majority must be a fraction such as 2/3" >&2; exit 2 ;;
esac
STOP_PRESENT=0
[ -e "$REPO/docs/evolution/STOP" ] && STOP_PRESENT=1

# Records mode starts with the parameters every later record was computed under.
if [ "$RECORDS" -eq 1 ]; then
    printf 'param\tboard_present\t1\n'
    printf 'param\trepo\t%s\n' "$REPO"
    printf 'param\tarea\t%s\n' "$AREA"
    printf 'param\tnow\t%s\n' "$NOW"
    printf 'param\tquorum\t%s\n' "$QUORUM"
    printf 'param\texpiry_days\t%s\n' "$EXPIRY_DAYS"
    printf 'param\tstale_days\t%s\n' "$STALE_DAYS"
    printf 'param\tamend_quorum\t%s\n' "$AMEND_QUORUM"
    printf 'param\tamend_majority\t%s/%s\n' "$AMEND_NUM" "$AMEND_DEN"
    printf 'param\tprotected\t%s\n' "$PROTECTED_ROW"
    printf 'param\tconstitution_present\t%d\n' "$([ -f "$CONSTITUTION" ] && echo 1 || echo 0)"
    printf 'param\tstop_present\t%d\n' "$STOP_PRESENT"
fi

LIST="$(mktemp)"
ALLROWS="$(mktemp)"
ROWS="$(mktemp)"
QUEUE="$(mktemp)"
AMEND="$(mktemp)"
AMEND_OUT="$(mktemp)"
CUR="$(mktemp)"
PROP="$(mktemp)"
BAL="$(mktemp)"
trap 'rm -f "$LIST" "$ALLROWS" "$ROWS" "$QUEUE" "$AMEND" "$AMEND_OUT" "$CUR" "$PROP" "$BAL"' EXIT

find "$BOARD" -mindepth 3 -maxdepth 3 -type f -name '*.md' | LC_ALL=C sort > "$LIST"
if [ ! -s "$LIST" ]; then
    [ "$RECORDS" -eq 1 ] && exit 0
    [ "$TSV" -eq 1 ] && exit 0
    echo "Board is empty (no posts under docs/board/<area>/<thread>/)."
    exit 0
fi

# One tab-separated row per post:
# path area thread id kind agent date expires on vote resolution summary
# session text_hash read
# "read" is 1 when the body has a line starting "Read:" (an informed vote).
tr '\n' '\0' < "$LIST" | xargs -0 awk '
function emit() {
    if (id == "") return
    gsub(/\t/, " ", v["summary"])
    printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%d\n", path, area, thread, id, \
        v["kind"], v["agent"], v["date"], v["expires"], v["on"], v["vote"], v["resolution"], v["summary"], \
        v["session"], v["text_hash"], has_read
}
FNR == 1 {
    emit()
    path = FILENAME
    state = 0
    has_read = 0
    split("", v)
    n = split(FILENAME, parts, "/")
    id = parts[n]; sub(/\.md$/, "", id)
    thread = parts[n - 1]
    area = parts[n - 2]
}
/^---[ \t]*$/ {
    if (state == 0) { state = 1; next }
    if (state == 1) { state = 2; next }
}
state == 1 && match($0, /^[a-z_]+:/) {
    key = substr($0, 1, RLENGTH - 1)
    val = substr($0, RLENGTH + 1)
    gsub(/^[ \t]+|[ \t]+$/, "", val)
    if (val ~ /^".*"$/) val = substr(val, 2, length(val) - 2)
    v[key] = val
}
state == 2 && /^Read:[ \t]*[^ \t]/ { has_read = 1 }
END { emit() }
' > "$ALLROWS"
awk -F'\t' -v a="$AREA" 'a == "" || $2 == a' "$ALLROWS" > "$ROWS"
if [ "$RECORDS" -eq 1 ]; then
    # Every post, repository-wide; the area filter applies to the computed
    # claim, thread, and suggestion records below, exactly as in text mode.
    sed 's/^/post'"$(printf '\t')"'/' "$ALLROWS"
fi

if [ ! -s "$ROWS" ] && [ "$RECORDS" -eq 0 ]; then
    [ "$TSV" -eq 1 ] || echo "No posts${AREA:+ in area \"$AREA\"}."
    # Amendments are repository-wide, so they are still shown below.
    if [ "$SECTION" = "ballot" ]; then
        :
    elif [ "$SECTION" = "all" ] || [ "$SECTION" = "amendments" ]; then
        SECTION=amendments
    else
        exit 0
    fi
fi

if [ "$TSV" -eq 1 ] && [ "$SECTION" != "ballot" ]; then
    cat "$ROWS"
    exit 0
fi

# The ballot reuses the two tallies below in records mode and formats the
# result afterwards, so its numbers can never drift from the text report.
AWK_RECORDS="$RECORDS"
[ "$SECTION" = "ballot" ] && AWK_RECORDS=1

# NOTICES: who is asking for a vote right now - here, on the registered brain
# (the PULSE framework checkout, docs/workflows/commons.md), and in the commons.
# Text mode only, before everything else, because it is the one block an
# agent must not miss. Counts come from --ballot --tsv of this same script
# run on each board, so the tally is still the only one.
ballot_counts() {
    # $1 = repository root, then extra flags; prints "<suggestions> <amendments>".
    local repo="$1" out
    shift
    out="$(bash "$HERE/board.sh" --repo "$repo" --now "$NOW" "$@" --ballot --tsv 2>/dev/null)" || return 1
    printf '%s\n' "$out" | awk -F'\t' 'BEGIN { s = 0; a = 0 } $1 == "suggestion" { s++ } $1 == "amendment" { a++ } END { printf "%d %d\n", s, a }'
}
plural() { [ "$1" -eq 1 ] && printf '%s' "$2" || printf '%ss' "$2"; }
if [ "$SECTION" = "all" ] && [ "$RECORDS" -eq 0 ] && [ "$NOTICES" -eq 1 ]; then
    PULSE_HOME_DIR="${PULSE_HOME:-$HOME/.pulse}"
    REGISTRY_FILE="$PULSE_HOME_DIR/projects.json"
    BRAIN_PATH=""; BRAIN_SLUG=""; COMMONS_PATH="${PULSE_COMMONS:-}"
    if [ -f "$REGISTRY_FILE" ] && command -v python3 >/dev/null 2>&1; then
        REG="$(python3 - "$REGISTRY_FILE" <<'PY' 2>/dev/null || true
import json, sys
try:
    data = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    data = {}
brain = next((p for p in data.get("projects", []) if isinstance(p, dict) and p.get("brain")), None)
print("%s\t%s\t%s" % (brain.get("path", "") if brain else "", brain.get("slug", "") if brain else "", data.get("commons") or ""))
PY
)"
        NTAB="$(printf '\t')"
        BRAIN_PATH="${REG%%"$NTAB"*}"; REST="${REG#*"$NTAB"}"; BRAIN_SLUG="${REST%%"$NTAB"*}"
        [ -n "$COMMONS_PATH" ] || COMMONS_PATH="${REST#*"$NTAB"}"
    fi
    [ -n "$COMMONS_PATH" ] || COMMONS_PATH="$PULSE_HOME_DIR/commons"
    HERE_REAL="$(cd "$REPO" && pwd -P)"
    IS_COMMONS=0
    [ -d "$COMMONS_PATH" ] && [ "$(cd "$COMMONS_PATH" && pwd -P)" = "$HERE_REAL" ] && IS_COMMONS=1
    echo "NOTICES (who is asking for a vote; one vote per identity, latest wins, no self-votes - docs/workflows/board.md)"
    if [ "$IS_COMMONS" -eq 1 ]; then
        echo "  this is the commons: posts here are data for every project; it has no queue and no loop"
    elif HERE_COUNTS="$(ballot_counts "$REPO" --quorum "$QUORUM" --expiry-days "$EXPIRY_DAYS" --amend-quorum "$AMEND_QUORUM" --amend-majority "$AMEND_MAJORITY")"; then
        set -- $HERE_COUNTS
        echo "  votes wanted here: $1 $(plural "$1" suggestion), $2 $(plural "$2" amendment) open - board.sh --ballot --agent <runner>/<model>"
    fi
    if [ -n "$BRAIN_PATH" ] && [ -d "$BRAIN_PATH/docs/board" ]; then
        if [ "$(cd "$BRAIN_PATH" && pwd -P)" = "$HERE_REAL" ]; then
            echo "  this repository is the brain: suggestions about PULSE itself belong on this board"
        elif BRAIN_COUNTS="$(ballot_counts "$BRAIN_PATH")"; then
            set -- $BRAIN_COUNTS
            echo "  brain ${BRAIN_SLUG:-?}: $1 $(plural "$1" suggestion), $2 $(plural "$2" amendment) open for votes - board.sh --repo $BRAIN_PATH --ballot"
        fi
    fi
    if [ -d "$COMMONS_PATH/docs/board" ]; then
        if [ "$IS_COMMONS" -eq 0 ]; then
            OPEN_Q="$(bash "$HERE/board.sh" --repo "$COMMONS_PATH" --now "$NOW" --records 2>/dev/null \
                | awk -F'\t' '$1 == "thread" && index($2, "questions/") == 1 && $5 == "open" { n++ } END { printf "%d", n + 0 }')"
            echo "  commons: $OPEN_Q open $(plural "$OPEN_Q" question) - docs/scripts/commons.sh --area questions"
        fi
    else
        echo "  commons: not created ($COMMONS_PATH; owner: docs/scripts/commons.sh --init)"
    fi
    # Proposals the agents agreed on that still wait for the human's yes or no
    # (repository-wide, like amendments): do not re-open the debate, remind the human.
    AWAITING="$(awk -F'\t' '{ k = $2 "/" $3; if ($11 != "") r[k] = $11 } END { n = 0; for (k in r) if (r[k] == "agreed") n++; printf "%d", n }' "$ALLROWS")"
    echo "  for the human: $AWAITING $(plural "$AWAITING" proposal) awaiting a yes or no - board.sh --proposals"
    echo ""
fi

if [ "$SECTION" != "amendments" ]; then
# Everything below is computed from the rows; the queue is sorted afterwards.
# In records mode the same tally prints typed records instead of text.
if [ "$SECTION" = "ballot" ]; then exec 3>"$BAL"; else exec 3>&1; fi
awk -F'\t' -v now="$NOW" -v quorum="$QUORUM" -v expiry_days="$EXPIRY_DAYS" \
    -v stale_days="$STALE_DAYS" -v section="$SECTION" -v queue_file="$QUEUE" -v records="$AWK_RECORDS" '
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
# One definition of each status, shared by the text report and the records.
function claim_status(id) {
    if (claim_key[id] in closed) return "closed"
    if (claim_exp[id] != "" && claim_exp[id] > now) return "active"
    if (claim_exp[id] == "") return "no expiry"
    return "expired"
}
function thread_status(key) {
    if (key in closed) return "closed"
    if (today_n - day_number(last[key]) > stale_days) return "stale"
    return "open"
}
# awaiting while the thread still ends on "agreed"; otherwise the recorded
# answer of the human (accepted, declined) or whatever resolution came after.
function proposal_status(key) {
    if (resolution[key] == "agreed") return "awaiting"
    return resolution[key]
}
{
    path = $1; area = $2; thread = $3; id = $4; kind = $5; agent = $6
    date = $7; expires = $8; on = $9; vote = $10; res = $11; summary = $12
    key = area "/" thread
    nposts[key]++
    if (!(key in last) || date >= last[key]) { last[key] = date; lastsum[key] = summary }
    if (res != "") { closed[key] = 1; resolution[key] = res; res_id[key] = id; res_date[key] = date }
    # A proposal for the human: the latest post in a thread that carries
    # resolution: agreed (the agents finished deliberating). It waits until a
    # later post records the answer of the human (accepted or declined).
    if (res == "agreed") {
        prop_id[key] = id; prop_agent[key] = agent; prop_date[key] = date; prop_sum[key] = summary
    }
    if (kind == "claim") {
        claim_key[id] = key; claim_agent[id] = agent; claim_exp[id] = expires; claim_sum[id] = summary
    }
    if (kind == "suggestion") {
        sugg_key[id] = key; sugg_agent[id] = agent; sugg_date[id] = date; sugg_sum[id] = summary
    }
    if (kind == "vote" && on != "") {
        vk = on SUBSEP agent
        if (!(vk in vote_date) || date >= vote_date[vk]) { vote_date[vk] = date; vote_val[vk] = vote; vote_id[vk] = id }
    }
    if (kind == "") nokind++
}
END {
    today_n = day_number(now)

    if (records) {
        printf "nokind\t%d\n", nokind + 0
        for (id in claim_key)
            printf "claim\t%s\t%s\t%s\t%s\t%s\t%s\n", id, claim_key[id], claim_agent[id], claim_exp[id], claim_status(id), claim_sum[id]
        for (key in nposts)
            printf "thread\t%s\t%d\t%s\t%s\t%s\t%s\n", key, nposts[key], last[key], thread_status(key), (key in closed ? resolution[key] : ""), lastsum[key]
        for (key in prop_id) {
            st = proposal_status(key)
            printf "proposal\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", key, prop_id[key], prop_agent[key], prop_date[key], st, \
                (st == "awaiting" ? "" : res_id[key]), (st == "awaiting" ? "" : res_date[key]), prop_sum[key]
        }
        for (vk in vote_val) {
            split(vk, p, SUBSEP)
            if (!(p[1] in sugg_key)) continue
            printf "svote\t%s\t%s\t%s\t%s\t%s\t%s\n", p[1], p[2], vote_id[vk], vote_date[vk], vote_val[vk], (p[2] == sugg_agent[p[1]] ? "self" : "counted")
        }
    }

    if (show("claims") && !records) {
        printf "ACTIVE CLAIMS (now %s)\n", now
        n = 0
        for (id in claim_key) {
            if (claim_status(id) == "active") {
                printf "  %-38s %-24s until %-20s %s\n", claim_key[id], trunc(claim_agent[id], 24), claim_exp[id], trunc(claim_sum[id], 60); n++
            }
        }
        if (n == 0) print "  none"
        print ""
    }

    if (show("threads") && !records) {
        print "OPEN THREADS"
        n = 0
        for (key in nposts) {
            if (thread_status(key) == "closed") continue
            printf "  %-38s %2d posts  last %-20s %s\n", key, nposts[key], last[key], trunc(lastsum[key], 60); n++
        }
        if (n == 0) print "  none"
        print ""
    }

    if (show("proposals") && !records) {
        print "PROPOSALS FOR THE HUMAN (the agents agreed; only the human decides - say yes or no in chat, and an agent records it as a result post with resolution: accepted or declined - docs/workflows/board.md)"
        n = 0; d = 0
        for (key in prop_id) {
            if (proposal_status(key) != "awaiting") { d++; continue }
            printf "  awaiting   %-38s since %-20s %s\n", key, prop_date[key], trunc(prop_sum[key], 60); n++
            printf "             read docs/board/%s/%s.md\n", key, prop_id[key]
        }
        for (key in prop_id) {
            st = proposal_status(key)
            if (st == "awaiting") continue
            printf "  %-10s %-38s on %-23s %s\n", st, key, res_date[key], trunc(prop_sum[key], 60)
        }
        if (n == 0 && d == 0) print "  none"
        print ""
    }

    # Queue rows go to a file so the shell can sort them by score, then date.
    # Records mode also tallies closed suggestions so their final count is
    # visible, and prints them directly (the reader sorts).
    for (id in sugg_key) {
        is_closed = (sugg_key[id] in closed)
        if (is_closed && !records) continue
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
        if (is_closed) status = "closed"
        else if (voters >= quorum) status = "eligible"
        else if (voters == 0 && age > expiry_days) status = "expired"
        else status = "needs votes"
        if (records)
            printf "suggestion\t%d\t%s\t%s\t%s\t%d\t%d\t%d\t%s\t%s\t%s\n", score, sugg_date[id], id, sugg_key[id], up, down, voters, status, sugg_agent[id], sugg_sum[id]
        else
            printf "%d\t%s\t%s\t%s\t%d\t%d\t%d\t%s\t%s\n", score, sugg_date[id], id, sugg_key[id], up, down, voters, status, sugg_sum[id] > queue_file
    }

    if (show("stale") && !records) {
        printf "STALE (expired claims, expired suggestions, threads idle > %d days)\n", stale_days
        n = 0
        for (id in claim_key) {
            st = claim_status(id)
            if (st == "expired" || st == "no expiry") {
                printf "  claim      %-38s %-24s expired %s\n", claim_key[id], trunc(claim_agent[id], 24), (claim_exp[id] == "" ? "(no expiry)" : claim_exp[id]); n++
            }
        }
        for (key in nposts) {
            if (thread_status(key) == "stale") {
                printf "  thread     %-38s idle since %s\n", key, last[key]; n++
            }
        }
        if (nokind > 0) { printf "  %d post(s) without a kind: prune them\n", nokind; n++ }
        if (n == 0) print "  none (expired suggestions, if any, are listed in the queue)"
        print ""
    }
}
' "$ROWS" >&3
exec 3>&-

if [ "$RECORDS" -eq 0 ] && { [ "$SECTION" = "all" ] || [ "$SECTION" = "queue" ] || [ "$SECTION" = "stale" ]; }; then
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
fi

if [ "$SECTION" != "all" ] && [ "$SECTION" != "amendments" ] && [ "$SECTION" != "ballot" ]; then
    exit 0
fi
[ "$SECTION" = "all" ] && [ "$RECORDS" -eq 0 ] && echo ""

# Constitutional amendments (docs/workflows/amend.md). Repository-wide, so the
# --area filter does not apply. For every open `amendment` post: hash its
# proposed text, check that the text it replaces still appears exactly once in
# the constitution and outside the protected sections, and refuse proposals
# that lower a threshold or drop a protection. Then count the latest vote per
# session, skipping the proposer's session, votes whose text_hash differs from
# the proposal, and votes without a "Read:" line.
TAB="$(printf '\t')"

awk -F'\t' '$5 == "amendment" { print $1 "\t" $4 }' "$ALLROWS" | while IFS="$TAB" read -r post id; do
    hash=""; anchor=""; section=""; verdict="malformed: no fenced block under \"## Proposed text\""
    if block "$post" "Proposed text" > "$PROP"; then
        hash="$(sha256 < "$PROP")"
        if ! block "$post" "Current text" > "$CUR"; then
            verdict="malformed: no fenced block under \"## Current text\""
        elif [ ! -f "$CONSTITUTION" ]; then
            verdict="unchecked: no docs/evolution/constitution.md"
        else
            checked="$(awk -v protected="$PROTECTED_ROW" '
            function cell(text, name,    re, c) {
                re = "\\|[ \t]*" name "[ \t]*\\|[^|]*"
                if (!match(text, re)) return "\001"
                c = substr(text, RSTART, RLENGTH); sub(/^[^|]*\|[^|]*\|/, "", c)
                gsub(/`/, "", c); gsub(/^[ \t]+|[ \t]+$/, "", c)
                return c
            }
            function num(text, name,    c, f) {
                c = cell(text, name)
                if (c == "\001") return ""
                if (match(c, /[0-9]+\/[0-9]+/)) { split(substr(c, RSTART, RLENGTH), f, "/"); return f[1] / f[2] }
                if (match(c, /[0-9]+/)) return substr(c, RSTART, RLENGTH) + 0
                return ""
            }
            function trim(s) { gsub(/^[ \t]+|[ \t]+$/, "", s); return s }
            FNR == 1 { which++ }
            which == 1 { cur = cur $0 "\n"; next }
            which == 2 { prop = prop $0 "\n"; next }
            { sub(/[ \t]+$/, ""); L[++n] = $0 }
            END {
                verdict = "ok"
                # Rows that guard the amendment procedure may be strengthened by
                # vote, never weakened or removed; weakening is a human commit.
                cq = num(cur, "Amendment quorum"); pq = num(prop, "Amendment quorum")
                if (cq != "" && (pq == "" || pq + 0 < cq + 0)) verdict = "lowers or removes the amendment quorum"
                cm = num(cur, "Amendment supermajority"); pm = num(prop, "Amendment supermajority")
                if (verdict == "ok" && cm != "" && (pm == "" || pm + 0 < cm + 0)) verdict = "lowers or removes the amendment supermajority"
                split("Amendment voter,Informed vote,Not amendable by vote", guard, ",")
                for (g = 1; g <= 3; g++) {
                    if (verdict == "ok" && cell(cur, guard[g]) != "\001" && cell(prop, guard[g]) == "\001")
                        verdict = "removes the \"" guard[g] "\" row"
                }
                cp = cell(cur, "Not amendable by vote"); pp = cell(prop, "Not amendable by vote")
                if (verdict == "ok" && cp != "\001" && pp != "\001") {
                    k = split(cp, items, ",")
                    for (i = 1; i <= k; i++) {
                        it = trim(items[i])
                        if (it != "" && index("," pp ",", it) == 0) verdict = "drops \"" it "\" from the protected sections"
                    }
                }
                # The replaced text must appear exactly once, as whole lines.
                hay = "\n"; for (i = 1; i <= n; i++) hay = hay L[i] "\n"
                needle = "\n" cur
                count = 0; pos = 1; first = 0
                while ((k = index(substr(hay, pos), needle)) > 0) {
                    count++
                    if (first == 0) first = pos + k - 1
                    pos += k
                }
                sections = ""
                if (count == 1) {
                    pre = substr(hay, 1, first); ln = gsub(/\n/, "\n", pre)
                    span = gsub(/\n/, "\n", cur)
                    fm_end = 0
                    if (L[1] ~ /^---[ \t]*$/) for (i = 2; i <= n; i++) if (L[i] ~ /^---[ \t]*$/) { fm_end = i; break }
                    sec = "preamble"
                    for (i = 1; i <= n; i++) {
                        if (L[i] ~ /^## /) { sec = substr(L[i], 4); sub(/[ \t]+$/, "", sec) }
                        S[i] = (fm_end > 0 && i <= fm_end) ? "frontmatter" : sec
                    }
                    gsub(/`/, "", protected); k = split(protected, items, ",")
                    for (j = ln; j < ln + span && j <= n; j++) {
                        if (index("|" sections "|", "|" S[j] "|") == 0) sections = (sections == "" ? S[j] : sections "|" S[j])
                        for (i = 1; i <= k; i++) {
                            it = trim(items[i])
                            if (verdict == "ok" && it != "" && tolower(it) == tolower(S[j])) verdict = "protected section \"" S[j] "\""
                        }
                    }
                }
                printf "%d\t%s\t%s\n", count, sections, verdict
            }' "$CUR" "$PROP" "$CONSTITUTION")"
            anchor="${checked%%"$TAB"*}"
            rest="${checked#*"$TAB"}"
            section="${rest%%"$TAB"*}"
            verdict="${rest#*"$TAB"}"
        fi
    fi
    printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$hash" "$anchor" "$section" "$verdict"
done > "$AMEND"

if [ "$RECORDS" -eq 0 ] && [ "$SECTION" != "ballot" ]; then
echo "AMENDMENTS (supermajority $AMEND_NUM/$AMEND_DEN of counted votes; quorum $AMEND_QUORUM distinct sessions, proposer excluded; a vote counts only with the proposal's text_hash and a Read: line)"
if [ ! -f "$CONSTITUTION" ]; then
    echo "  no docs/evolution/constitution.md: nothing to amend"
fi
if [ "$STOP_PRESENT" -eq 1 ]; then
    echo "  docs/evolution/STOP is present: ratification is paused until a human removes it"
fi
fi
# The tally below is the only one; records mode prints the same numbers as
# typed records (amendment, avote) and also tallies closed amendments.
AMEND_AWK='
function trunc(s, n) { return length(s) > n ? substr(s, 1, n - 1) "~" : s }
FILENAME == amend_file { am_hash[$1] = $2; am_anchor[$1] = $3; am_section[$1] = $4; am_verdict[$1] = $5; next }
{
    area = $2; thread = $3; id = $4; kind = $5; agent = $6; date = $7; on = $9; vote = $10
    res = $11; summary = $12; session = $13; thash = $14; rd = $15
    key = area "/" thread
    if (res != "") { closed[key] = 1; resolution[key] = res }
    if (kind == "amendment") {
        amend_key[id] = key; amend_session[id] = session; amend_date[id] = date; amend_sum[id] = summary; amend_agent[id] = agent
    }
    if (kind == "vote" && on != "") {
        vk = on SUBSEP session
        if (!(vk in vote_date) || date >= vote_date[vk]) {
            vote_date[vk] = date; vote_val[vk] = vote; vote_hash[vk] = thash; vote_read[vk] = rd; vote_agent[vk] = agent; vote_id[vk] = id
        }
    }
}
END {
    n = 0
    for (id in amend_key) {
        is_closed = (amend_key[id] in closed)
        if (is_closed && !records) continue
        if (!is_closed) n++
        up = 0; down = 0; voters = 0; self = 0; badhash = 0; noread = 0
        split("", idents)
        for (vk in vote_val) {
            split(vk, p, SUBSEP)
            if (p[1] != id) continue
            if (p[2] == amend_session[id] || p[2] == "") { self++; cls = "self" }
            else if (vote_hash[vk] != am_hash[id]) { badhash++; cls = "hash mismatch" }
            else if (vote_read[vk] != 1) { noread++; cls = "no Read line" }
            else {
                voters++
                if (vote_val[vk] == "down") down++; else up++
                idents[vote_agent[vk]] = 1
                cls = "counted"
            }
            if (records) printf "avote\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", id, p[2], vote_agent[vk], vote_id[vk], vote_date[vk], vote_val[vk], cls
        }
        nid = 0; for (a in idents) nid++
        total = up + down
        pct = total > 0 ? int(100 * up / total + 0.5) : 0
        verdict = am_verdict[id]
        if (!(id in am_hash) || am_hash[id] == "") status = "malformed"
        else if (verdict ~ /^malformed/ || verdict ~ /^unchecked/) status = verdict
        else if (verdict != "ok") status = "blocked: " verdict
        else if (am_anchor[id] + 0 == 0) status = "stale: the replaced text is no longer in the constitution; re-propose"
        else if (am_anchor[id] + 0 > 1) status = "ambiguous: the replaced text appears " am_anchor[id] " times; quote more context"
        else if (voters < quorum) status = "needs votes"
        else if (up * den >= num * total) status = (stop == 1 ? "passed; ratification paused (STOP)" : "passed: ratify per docs/workflows/amend.md")
        else status = "short of the supermajority"
        if (records) {
            if (is_closed) status = "closed: " resolution[amend_key[id]]
            printf "amendment\t%s\t%s\t%s\t%s\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%s\t%s\t%s\t%s\n", amend_date[id], id, amend_key[id], status, up, down, voters, quorum, pct, nid, self, badhash, noread, am_hash[id], am_section[id], amend_agent[id], amend_sum[id]
            continue
        }
        out = sprintf("  %-38s %s", amend_key[id], trunc(id, 60))
        out = out "\001" sprintf("      %s", status)
        out = out "\001" sprintf("      up %d down %d  voters %d/%d  %d%% up  identities %d  uncounted: %d self, %d hash mismatch, %d no Read line", up, down, voters, quorum, pct, nid, self, badhash, noread)
        if (am_hash[id] != "") out = out "\001" sprintf("      text_hash %s  section %s", am_hash[id], (am_section[id] == "" ? "-" : am_section[id]))
        out = out "\001" sprintf("      %s", trunc(amend_sum[id], 90))
        printf "%s\t%s\t%s\n", amend_date[id], id, out
    }
    if (n == 0 && !records) print "\t\t  none open"
}'
if [ "$RECORDS" -eq 1 ]; then
    awk -F'\t' -v quorum="$AMEND_QUORUM" -v num="$AMEND_NUM" -v den="$AMEND_DEN" -v stop="$STOP_PRESENT" -v amend_file="$AMEND" -v records=1 "$AMEND_AWK" "$AMEND" "$ALLROWS"
    exit 0
fi

# The ballot: the records both tallies printed above, filtered to what is
# still open and, when an identity is given, to what it can still vote on.
if [ "$SECTION" = "ballot" ]; then
    awk -F'\t' -v quorum="$AMEND_QUORUM" -v num="$AMEND_NUM" -v den="$AMEND_DEN" -v stop="$STOP_PRESENT" -v amend_file="$AMEND" -v records=1 "$AMEND_AWK" "$AMEND" "$ALLROWS" >> "$BAL"
    if [ "$TSV" -eq 0 ]; then
        if [ -n "$BALLOT_AGENT" ]; then
            echo "BALLOT for $BALLOT_AGENT (open items by other identities without your vote; vote up or down with the reason in the vote's summary - docs/workflows/board.md; an amendment vote also needs text_hash and a Read: line - docs/workflows/amend.md)"
        else
            echo "BALLOT (every open suggestion and amendment; add --agent <runner>/<model> to hide your own and those you voted on)"
        fi
    fi
    awk -F'\t' -v me="$BALLOT_AGENT" -v tsv="$TSV" '
    function trunc(s, n) { return length(s) > n ? substr(s, 1, n - 1) "~" : s }
    $1 == "suggestion" {
        if ($9 == "closed" || $9 == "expired") next
        s_date[$4] = $3; s_key[$4] = $5; s_up[$4] = $6; s_down[$4] = $7; s_voters[$4] = $8; s_status[$4] = $9; s_agent[$4] = $10; s_sum[$4] = $11
        next
    }
    $1 == "svote" { if ($3 == me) voted[$2] = 1; next }
    $1 == "amendment" {
        if ($5 ~ /^closed/) next
        a_date[$3] = $2; a_key[$3] = $4; a_status[$3] = $5; a_up[$3] = $6; a_down[$3] = $7; a_voters[$3] = $8; a_quorum[$3] = $9
        a_hash[$3] = $15; a_agent[$3] = $17; a_sum[$3] = $18
        next
    }
    $1 == "avote" { if ($4 == me) voted[$2] = 1; next }
    END {
        n = 0
        for (id in s_key) {
            if (me != "" && (s_agent[id] == me || (id in voted))) continue
            n++
            if (tsv) printf "1\t%s\tsuggestion\t%s\t%s\t%s\t%d\t%d\t%d\t%s\t\t%s\n", s_date[id], s_status[id], s_key[id], id, s_up[id], s_down[id], s_voters[id], s_agent[id], s_sum[id]
            else printf "1\t%s\t  suggestion  %-12s %-38s %-44s up %d down %d voters %d  %s\n", s_date[id], s_status[id], s_key[id], trunc(id, 44), s_up[id], s_down[id], s_voters[id], trunc(s_sum[id], 50)
        }
        for (id in a_key) {
            if (me != "" && (a_agent[id] == me || (id in voted))) continue
            n++
            st = a_status[id]
            if (st ~ /^blocked/) st = "blocked"; else if (st ~ /^stale/) st = "stale"; else if (st ~ /^ambiguous/) st = "ambiguous"
            else if (st ~ /^passed/) st = "passed"; else if (st ~ /^short/) st = "short"; else if (st ~ /^malformed|^unchecked/) st = "malformed"
            if (tsv) printf "2\t%s\tamendment\t%s\t%s\t%s\t%d\t%d\t%d\t%s\t%s\t%s\n", a_date[id], st, a_key[id], id, a_up[id], a_down[id], a_voters[id], a_agent[id], a_hash[id], a_sum[id]
            else printf "2\t%s\t  amendment   %-12s %-38s %-44s voters %d/%d  text_hash %s  %s\n", a_date[id], st, a_key[id], trunc(id, 44), a_voters[id], a_quorum[id], (a_hash[id] == "" ? "-" : substr(a_hash[id], 1, 12) "~"), trunc(a_sum[id], 50)
        }
        if (n == 0 && !tsv) print "0\t\t  none - nothing open awaits " (me == "" ? "a vote" : "your vote")
    }' "$BAL" | sort -t "$TAB" -k1,1n -k2,2 | cut -f3-
    exit 0
fi
awk -F'\t' -v quorum="$AMEND_QUORUM" -v num="$AMEND_NUM" -v den="$AMEND_DEN" -v stop="$STOP_PRESENT" -v amend_file="$AMEND" -v records=0 "$AMEND_AWK" "$AMEND" "$ALLROWS" > "$AMEND_OUT"
sort -t "$TAB" -k1,1 -k2,2 "$AMEND_OUT" | cut -f3- | tr '\001' '\n'
