#!/usr/bin/env bash
# Verify committed (HEAD) gates for the evolution loop.
# Usage: docs/scripts/verify-committed-gates.sh REPO
set -eu
repo="$1"
cd "$repo"
failed=0
out_lines=()
# Ensure this is a git repo and HEAD exists
if ! git rev-parse --verify HEAD >/dev/null 2>&1; then
  out_lines+=("not a git repository or no HEAD")
  failed=1
fi
# Check for uncommitted changes (helpful diagnostic; not a failure)
if git status --porcelain | grep -q .; then
  out_lines+=("working tree: has uncommitted changes (commit them for an accurate HEAD check)")
else
  out_lines+=("working tree: clean")
fi
# Check for STOP in HEAD
if git cat-file -e HEAD:docs/evolution/STOP 2>/dev/null; then
  out_lines+=("KILL SWITCH: docs/evolution/STOP is present in HEAD")
  failed=1
else
  out_lines+=("KILL SWITCH: not present in HEAD")
fi
# Check for constitution.md in HEAD
if git cat-file -e HEAD:docs/evolution/constitution.md 2>/dev/null; then
  out_lines+=("constitution: present in HEAD")
else
  out_lines+=("constitution: MISSING in HEAD (docs/evolution/constitution.md not committed)")
  failed=1
fi
# Check evaluators baseline and archive row in HEAD
baseline=""
if git cat-file -e HEAD:docs/evolution/evaluators.md 2>/dev/null; then
  # extract baseline from frontmatter in the committed file
  baseline=$(git show HEAD:docs/evolution/evaluators.md | awk '\
    NR == 1 && !/^---/ { exit } \n\
    /^---/ { c++; if (c == 2) exit; next } \n\
    c == 1 && index($0, "baseline:") == 1 { v = substr($0, length("baseline:") + 2); gsub(/^[ \t]+|[ \t]+$/, "", v); print v; exit }\
  ')
  if [ -z "$baseline" ]; then
    out_lines+=("evaluators: present in HEAD, baseline frontmatter: unset")
    failed=1
  else
    out_lines+=("evaluators: present in HEAD, baseline: $baseline")
    # check archive row
    if git cat-file -e HEAD:docs/evolution/archive.md 2>/dev/null; then
      if git show HEAD:docs/evolution/archive.md | grep -q "^| $baseline "; then
        out_lines+=("archive: baseline row for '$baseline' found in HEAD")
      else
        out_lines+=("archive: NO baseline row for '$baseline' in HEAD")
        failed=1
      fi
    else
      out_lines+=("archive: MISSING in HEAD (docs/evolution/archive.md not committed)")
      failed=1
    fi
  fi
else
  out_lines+=("evaluators: MISSING in HEAD (docs/evolution/evaluators.md not committed)")
  failed=1
fi
# Print results
for l in "${out_lines[@]}"; do
  echo "$l"
done
if [ "$failed" -eq 0 ]; then
  exit 0
else
  exit 1
fi
