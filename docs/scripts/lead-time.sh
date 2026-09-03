#!/usr/bin/env bash
# Read-only lead-time report for PULSE artifacts.
#
# Reads the frontmatter of docs/decisions/ADR-*.md, docs/plans/*.md (Tier 2
# plans and Tier 1 work units), and docs/features/NNN-*.md, then reports
# proposed -> delivered days per artifact from the `date:` and `delivered:`
# fields. It never writes to the repository.
#
# Targets: Tier 1 within two days. Tier 0 has no artifact, so its same-day
# target is checked at the quarterly prune from commits, not here. Tier 2 has
# no fixed target and is reviewed at each prune.
#
# Usage: docs/scripts/lead-time.sh [--repo PATH] [--today YYYY-MM-DD] [--open] [--tsv]
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/lead-time.sh [--repo PATH] [--today YYYY-MM-DD] [--open] [--tsv]

Read-only report of proposed -> delivered days for ADRs, plans, work units,
and feature specs, based on the frontmatter `date:` and `delivered:` fields.

  --repo PATH     Repository root (default: the repository containing this script)
  --today DATE    Date used for the age of open artifacts (default: today)
  --open          Show only artifacts that are not delivered or retired
  --tsv           Emit tab-separated rows only, without the summary
  -h, --help      Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
TODAY="$(date +%Y-%m-%d)"
ONLY_OPEN=0
TSV=0

while [ $# -gt 0 ]; do
    case "$1" in
        --repo)
            [ $# -ge 2 ] || { usage >&2; exit 2; }
            REPO="$(cd "$2" && pwd)"
            shift 2
            ;;
        --today)
            [ $# -ge 2 ] || { usage >&2; exit 2; }
            TODAY="$2"
            shift 2
            ;;
        --open) ONLY_OPEN=1; shift ;;
        --tsv) TSV=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

case "$TODAY" in
    [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]) ;;
    *) echo "lead-time: --today must be YYYY-MM-DD" >&2; exit 2 ;;
esac

# One tab-separated row per artifact:
# kind, id, tier, status, delivery, date, delivered, path
collect() {
    for f in "$REPO"/docs/decisions/ADR-*.md \
             "$REPO"/docs/plans/*.md \
             "$REPO"/docs/features/[0-9]*.md; do
        [ -f "$f" ] || continue
        case "$(basename "$f")" in
            _template*|README.md) continue ;;
        esac
        awk -v path="${f#"$REPO"/}" '
            NR == 1 && $0 == "---" { fm = 1; next }
            fm == 1 && $0 == "---" { fm = 2; exit }
            fm == 1 {
                i = index($0, ":")
                if (i == 0) next
                k = substr($0, 1, i - 1)
                v = substr($0, i + 1)
                gsub(/^[ \t]+|[ \t]+$/, "", k)
                gsub(/^[ \t]+|[ \t]+$/, "", v)
                gsub(/^"|"$/, "", v)
                m[k] = v
            }
            END {
                if (fm == 0) exit
                kind = m["id"]
                sub(/-.*/, "", kind)
                if (kind == "") {
                    if (path ~ /^docs\/decisions\//) kind = "ADR"
                    else if (path ~ /^docs\/features\//) kind = "FEAT"
                    else if (m["tags"] ~ /work-unit/) kind = "WU"
                    else kind = "PLAN"
                }
                tier = m["tier"]
                if (tier == "" && kind == "ADR") tier = "2"
                printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", kind, m["id"], tier,
                    m["status"], m["delivery"], m["date"], m["delivered"], path
            }' "$f"
    done
}

collect | sort -t "$(printf '\t')" -k1,1 -k2,2 | awk -F '\t' -v today="$TODAY" -v only_open="$ONLY_OPEN" -v tsv="$TSV" '
    # Days since 1970-01-01 for a proleptic Gregorian date (civil-from-days).
    function days_from_civil(y, m, d,    era, yoe, doy, doe) {
        if (m <= 2) y -= 1
        era = int(y / 400)
        yoe = y - era * 400
        doy = int((153 * (m > 2 ? m - 3 : m + 9) + 2) / 5) + d - 1
        doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
        return era * 146097 + doe - 719468
    }
    function day_number(s,    a) {
        if (s !~ /^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$/) return ""
        split(s, a, "-")
        return days_from_civil(a[1] + 0, a[2] + 0, a[3] + 0)
    }
    function limit_for(tier) {
        if (tier == "0") return 0
        if (tier == "1") return 2
        return -1
    }
    function median(arr, n,    i, j, t, sorted) {
        for (i = 1; i <= n; i++) sorted[i] = arr[i]
        for (i = 2; i <= n; i++) {
            t = sorted[i]
            for (j = i - 1; j >= 1 && sorted[j] > t; j--) sorted[j + 1] = sorted[j]
            sorted[j + 1] = t
        }
        if (n % 2) return sorted[(n + 1) / 2]
        return (sorted[n / 2] + sorted[n / 2 + 1]) / 2
    }
    function short(s, n) { return (length(s) > n) ? substr(s, 1, n - 1) "~" : s }
    BEGIN {
        today_n = day_number(today)
        if (!tsv) {
            printf "%-5s %-9s %-4s %-22s %-19s %-10s %-10s %5s  %s\n",
                "KIND", "ID", "TIER", "STATUS", "DELIVERY", "PROPOSED", "DELIVERED", "DAYS", "TARGET"
        }
    }
    {
        kind = $1; id = $2; tier = $3; status = $4; delivery = $5
        proposed = $6; delivered = $7; path = $8
        p = day_number(proposed); d = day_number(delivered)
        retired = (delivery == "retired" || status ~ /^(Superseded|Deprecated)/)
        closed = (delivery == "delivered" || status ~ /^Done/)
        if (p == "") { state = "no-date"; days = "?" }
        else if (d != "") { state = "delivered"; days = d - p }
        else if (retired) { state = "retired"; days = "-" }
        else if (closed) { state = "undated"; days = "?" }
        else { state = "open"; days = today_n - p }
        limit = limit_for(tier)
        if (state == "delivered") target = (limit < 0) ? "-" : ((days <= limit) ? "met" : "missed")
        else if (state == "open") target = (limit < 0) ? "open" : ((days <= limit) ? "open" : "overdue")
        else if (state == "retired") target = "retired"
        else if (state == "undated") target = "add delivered:"
        else target = "add date:"
        if (only_open && state != "open") next
        total++
        if (state == "delivered") {
            n_delivered[tier]++
            sum_days[tier] += days
            all_days[tier, n_delivered[tier]] = days
            if (target == "met") met[tier]++
            if (target == "missed") missed[tier]++
        } else if (state == "open") {
            n_open[tier]++
            if (days > oldest[tier]) oldest[tier] = days
            if (target == "overdue") overdue[tier]++
        } else if (state == "retired") n_retired++
        else if (state == "undated") n_undated++
        else n_nodate++
        show_status = status
        sub(/ *\(.*$/, "", show_status)
        if (tsv) {
            printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n",
                kind, id, tier, status, delivery, proposed, delivered, days, target, path
        } else {
            printf "%-5s %-9s %-4s %-22s %-19s %-10s %-10s %5s  %s\n",
                kind, id, (tier == "" ? "-" : tier), short(show_status, 22), short(delivery == "" ? "-" : delivery, 19),
                (proposed == "" ? "-" : proposed), (delivered == "" ? "-" : delivered), days, target
        }
    }
    END {
        if (tsv) exit
        print ""
        if (total == 0) {
            print (only_open ? "No open artifacts: everything proposed is delivered or retired." : "No artifacts with frontmatter found.")
            exit
        }
        print "Summary (proposed -> delivered days from frontmatter; read-only)"
        for (t = 0; t <= 2; t++) {
            tier = t ""
            if (!(tier in n_delivered) && !(tier in n_open)) continue
            parts = ""
            if (tier in n_delivered) {
                k = n_delivered[tier]
                for (i = 1; i <= k; i++) tmp[i] = all_days[tier, i]
                parts = sprintf("%d delivered (median %.1f, mean %.1f days)", k, median(tmp, k), sum_days[tier] / k)
                lim = limit_for(tier)
                if (lim >= 0) parts = parts sprintf(", target <= %d days: %d met, %d missed", lim, met[tier] + 0, missed[tier] + 0)
            }
            if (tier in n_open) {
                open_part = sprintf("%d open (oldest %d days", n_open[tier], oldest[tier] + 0)
                if (limit_for(tier) >= 0) open_part = open_part sprintf(", %d overdue", overdue[tier] + 0)
                open_part = open_part ")"
                parts = (parts == "") ? open_part : parts "; " open_part
            }
            printf "  Tier %s: %s\n", tier, parts
        }
        if (("" in n_delivered) || ("" in n_open)) {
            print "  Untiered artifacts: add `tier:` to their frontmatter so targets can apply."
        }
        if (n_retired) printf "  Retired: %d\n", n_retired
        if (n_undated) printf "  Closed without a `delivered:` date: %d (add it at the next prune)\n", n_undated
        if (n_nodate) printf "  Missing or invalid `date:`: %d\n", n_nodate
        print "  Tier 0 has no artifact; check its same-day target from commits at the prune."
    }'
