#!/usr/bin/env bash
# Prepare, verify, and clean isolated clones for a multi-agent fleet.
#
# A fleet is one orchestrating agent plus workers that each act inside their
# own throwaway clone: detached at one revision, no remote, no tags, with any
# answer-key files stripped. Workers never touch the real repository; the
# orchestrator scores their results read-only. This script writes only inside
# the destination directory it is given and refuses a destination inside the
# repository. It never pushes, fetches, or changes the source repository.
#
# Usage:
#   docs/scripts/fleet.sh prepare --dest DIR --rev REV --count N [--label L] [--strip PATH]... [--repo PATH]
#   docs/scripts/fleet.sh verify  --dest DIR
#   docs/scripts/fleet.sh clean   --dest DIR
set -eu

usage() {
    cat <<'EOF'
Usage:
  docs/scripts/fleet.sh prepare --dest DIR --rev REV --count N [--label L] [--strip PATH]... [--repo PATH]
  docs/scripts/fleet.sh verify  --dest DIR
  docs/scripts/fleet.sh clean   --dest DIR

prepare  Create N isolated clones DIR/<label>-<i>, each detached at REV with
         no remote and no tags, delete every --strip PATH inside the clone and
         commit that deletion locally, then record the clone in DIR/manifest.tsv.
verify   Report, for every clone in the manifest: HEAD, remotes, tags,
         stripped paths still absent, and uncommitted changes.
clean    Delete only the clones listed in DIR/manifest.tsv, then the manifest.

  --dest DIR     Fleet directory, outside the repository (required)
  --rev REV      Commit, tag, or branch to detach at (prepare; required)
  --count N      Number of clones (prepare; default: 1)
  --label L      Clone name prefix (prepare; default: the revision's short hash)
  --strip PATH   Repository-relative path to remove from each clone (repeatable)
  --repo PATH    Source repository (default: the repository containing this script)
  -h, --help     Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
CMD=""
DEST=""
REV=""
COUNT=1
LABEL=""
STRIP=""

[ $# -ge 1 ] || { usage >&2; exit 2; }
case "$1" in
    prepare|verify|clean) CMD="$1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac

while [ $# -gt 0 ]; do
    case "$1" in
        --dest)  [ $# -ge 2 ] || { usage >&2; exit 2; }; DEST="$2"; shift 2 ;;
        --rev)   [ $# -ge 2 ] || { usage >&2; exit 2; }; REV="$2"; shift 2 ;;
        --count) [ $# -ge 2 ] || { usage >&2; exit 2; }; COUNT="$2"; shift 2 ;;
        --label) [ $# -ge 2 ] || { usage >&2; exit 2; }; LABEL="$2"; shift 2 ;;
        --strip) [ $# -ge 2 ] || { usage >&2; exit 2; }; STRIP="$STRIP $2"; shift 2 ;;
        --repo)  [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

[ -n "$DEST" ] || { echo "fleet: --dest is required" >&2; exit 2; }

# The destination must never be inside the source repository. Resolve the
# parent that already exists so the check runs before anything is created.
REPO_REAL="$(cd "$REPO" && pwd -P)"
probe="$DEST"
while [ ! -d "$probe" ]; do
    parent="$(dirname "$probe")"
    [ "$parent" = "$probe" ] && break
    probe="$parent"
done
PROBE_REAL="$(cd "$probe" && pwd -P)"
case "$PROBE_REAL/" in
    "$REPO_REAL"/*) echo "fleet: refusing --dest inside the repository ($DEST)" >&2; exit 2 ;;
esac
if [ "$CMD" = "prepare" ]; then
    mkdir -p "$DEST"
elif [ ! -d "$DEST" ]; then
    echo "fleet: no fleet directory at $DEST" >&2; exit 2
fi
DEST_REAL="$(cd "$DEST" && pwd -P)"
MANIFEST="$DEST_REAL/manifest.tsv"

case "$CMD" in
prepare)
    [ -n "$REV" ] || { echo "fleet: --rev is required for prepare" >&2; exit 2; }
    case "$COUNT" in ''|*[!0-9]*) echo "fleet: --count must be a number" >&2; exit 2 ;; esac
    [ "$COUNT" -ge 1 ] || { echo "fleet: --count must be at least 1" >&2; exit 2; }
    git -C "$REPO_REAL" rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo "fleet: $REPO_REAL is not a git repository" >&2; exit 2; }
    COMMIT="$(git -C "$REPO_REAL" rev-parse --verify --quiet "$REV^{commit}")" || { echo "fleet: cannot resolve --rev $REV" >&2; exit 2; }
    SHORT="$(git -C "$REPO_REAL" rev-parse --short "$COMMIT")"
    [ -n "$LABEL" ] || LABEL="$SHORT"
    for p in $STRIP; do
        git -C "$REPO_REAL" cat-file -e "$COMMIT:$p" 2>/dev/null || echo "fleet: note - $p does not exist at $REV; nothing to strip" >&2
    done
    i=1
    while [ "$i" -le "$COUNT" ]; do
        clone="$DEST_REAL/$LABEL-$i"
        if [ -e "$clone" ]; then
            echo "fleet: $clone already exists; skipping" >&2
            i=$((i + 1)); continue
        fi
        git clone --quiet --no-tags --no-checkout "$REPO_REAL" "$clone"
        git -C "$clone" remote remove origin
        git -C "$clone" checkout --quiet --detach "$COMMIT"
        stripped=""
        for p in $STRIP; do
            if [ -e "$clone/$p" ]; then
                git -C "$clone" rm --quiet -r "$p"
                stripped="$stripped $p"
            fi
        done
        if [ -n "$stripped" ]; then
            git -C "$clone" -c user.name=fleet -c user.email=fleet@localhost -c commit.gpgsign=false \
                commit --quiet -m "fleet: strip answer keys for this worker"
        fi
        for t in $(git -C "$clone" tag -l); do git -C "$clone" tag -d "$t" >/dev/null; done
        printf '%s\t%s\t%s\t%s\t%s\n' "$clone" "$COMMIT" "$REV" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${stripped# }" >> "$MANIFEST"
        echo "$clone"
        i=$((i + 1))
    done
    echo "fleet: $COUNT clone(s) at $REV ($SHORT) under $DEST_REAL; manifest $MANIFEST" >&2
    ;;
verify)
    [ -f "$MANIFEST" ] || { echo "fleet: no manifest at $MANIFEST" >&2; exit 2; }
    status=0
    while IFS="$(printf '\t')" read -r clone commit rev created stripped; do
        [ -n "$clone" ] || continue
        if [ ! -d "$clone/.git" ]; then echo "MISSING  $clone"; status=1; continue; fi
        head="$(git -C "$clone" rev-parse HEAD)"
        remotes="$(git -C "$clone" remote | tr '\n' ' ')"
        tags="$(git -C "$clone" tag -l | tr '\n' ' ')"
        dirty="$(git -C "$clone" status --porcelain | wc -l | tr -d ' ')"
        ok="ok"
        if [ -n "$stripped" ]; then
            # HEAD is the strip commit; its parent must be the requested commit
            parent="$(git -C "$clone" rev-parse HEAD^ 2>/dev/null || echo none)"
            [ "$parent" = "$commit" ] || ok="HEAD moved"
            for p in $stripped; do [ -e "$clone/$p" ] && ok="stripped path present: $p"; done
        else
            [ "$head" = "$commit" ] || ok="HEAD moved"
        fi
        [ -z "$remotes" ] || ok="remote present: $remotes"
        [ -z "$tags" ] || ok="tags present: $tags"
        [ "$ok" = "ok" ] || status=1
        printf '%-8s %s  rev=%s  changed=%s%s\n' "$ok" "$clone" "$rev" "$dirty" "$([ -n "$stripped" ] && printf '  stripped=%s' "$stripped")"
    done < "$MANIFEST"
    exit "$status"
    ;;
clean)
    [ -f "$MANIFEST" ] || { echo "fleet: no manifest at $MANIFEST; nothing removed" >&2; exit 0; }
    while IFS="$(printf '\t')" read -r clone commit rev created stripped; do
        [ -n "$clone" ] || continue
        case "$clone" in
            "$DEST_REAL"/*) rm -rf "$clone"; echo "removed $clone" ;;
            *) echo "fleet: refusing to remove $clone (outside $DEST_REAL)" >&2 ;;
        esac
    done < "$MANIFEST"
    rm -f "$MANIFEST"
    rmdir "$DEST_REAL" 2>/dev/null || true
    ;;
esac
