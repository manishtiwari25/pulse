#!/usr/bin/env bash
# Read-only relative-link check for the PULSE control plane.
#
# Scans every Markdown file in the repository for Markdown links and images
# whose target is a relative path, resolves each against the file's folder,
# and reports targets that do not exist. External links, mailto links,
# in-page anchors, and template placeholders (targets containing NNN or angle
# brackets) are skipped. It never writes to the repository.
#
# Usage: docs/scripts/check-links.sh [--repo PATH] [--quiet]
# Exit status: 0 when every link resolves, 1 when any is broken.
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/check-links.sh [--repo PATH] [--quiet]

Read-only check that relative Markdown links and images resolve to files.

  --repo PATH   Repository root (default: the repository containing this script)
  --quiet       Print only broken links and the summary line
  -h, --help    Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
QUIET=0

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        --quiet) QUIET=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

BROKEN="$(mktemp)"
trap 'rm -f "$BROKEN"' EXIT
files=0
links=0

while IFS= read -r -d '' f; do
    files=$((files + 1))
    dir="$(dirname "$f")"
    while IFS="$(printf '\t')" read -r ln target; do
        links=$((links + 1))
        case "$target" in
            http://*|https://*|mailto:*|\#*) continue ;;
        esac
        target="${target%%#*}"
        target="${target%% *}"
        [ -n "$target" ] || continue
        case "$target" in
            *NNN*|*\<*|*\>*) continue ;;
        esac
        if [ ! -e "$dir/$target" ]; then
            printf '%s:%s -> %s\n' "${f#"$REPO"/}" "$ln" "$target" >> "$BROKEN"
        fi
    done < <(awk '{
        line = $0
        while (match(line, /\]\([^)]+\)/)) {
            printf "%d\t%s\n", NR, substr(line, RSTART + 2, RLENGTH - 3)
            line = substr(line, RSTART + RLENGTH)
        }
    }' "$f")
done < <(find "$REPO" -name '*.md' -not -path '*/.git/*' -not -path '*/node_modules/*' -print0 | LC_ALL=C sort -z)

count=0
if [ -s "$BROKEN" ]; then
    count="$(wc -l < "$BROKEN" | tr -d ' ')"
    sed 's/^/BROKEN /' "$BROKEN"
fi
if [ "$QUIET" -eq 0 ] || [ "$count" -gt 0 ]; then
    echo "Checked $links link(s) in $files Markdown file(s); $count broken."
fi
[ "$count" -eq 0 ]
