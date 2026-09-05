#!/usr/bin/env bash
# Read-only digest for the pulse-reflect skill.
#
# Reports the repository's working state, the board summary, the loop's gates,
# the product scope (whether the loop may improve the product the repository
# ships, or is confined to the operating layer), hygiene (stray evolve/*
# branches and worktrees the loop must remove), cheap health checks, and the
# highest level of the reflection ladder the gates allow right now:
#   A observe   always
#   B record    docs/board/ exists
#   C rehearse  docs/evolution/ filled, baseline recorded, no STOP
#   D evolve    C plus loop: on, held-out set listed, and no cycle cap reached
#               (none exists with limits: none, PULSE's default)
# It never writes to the repository and always exits 0; failed checks are
# printed, not raised, because the digest is input for an agent, not a gate.
#
# Usage: docs/scripts/reflect.sh [--repo PATH] [--area AREA] [--agent ID] [--no-board]
set -eu

usage() {
    cat <<'EOF'
Usage: docs/scripts/reflect.sh [--repo PATH] [--area AREA] [--agent ID] [--no-board]

Read-only digest for pulse-reflect: git state, board summary, your ballot,
loop gates, product scope, hygiene, health checks, the level the gates allow,
and the next human step.

  --repo PATH   Repository root (default: the repository containing this script)
  --area AREA   Limit the board summary to one channel
  --agent ID    Your identity (<runner>/<model>; default: $PULSE_AGENT); the ballot
                then hides what you authored or already voted on
  --no-board    Skip the board summary (it is already on screen)
  -h, --help    Show this help
EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
AREA=""
AGENT="${PULSE_AGENT:-}"
SHOW_BOARD=1

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) [ $# -ge 2 ] || { usage >&2; exit 2; }; REPO="$(cd "$2" && pwd)"; shift 2 ;;
        --area) [ $# -ge 2 ] || { usage >&2; exit 2; }; AREA="$2"; shift 2 ;;
        --agent) [ $# -ge 2 ] || { usage >&2; exit 2; }; AGENT="$2"; shift 2 ;;
        --no-board) SHOW_BOARD=0; shift ;;
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 2 ;;
    esac
done

SCRIPTS="$REPO/docs/scripts"
EVO="$REPO/docs/evolution"
BOARD="$REPO/docs/board"
TODAY="$(date -u +%Y-%m-%d)"

frontmatter_value() {
    # frontmatter_value FILE KEY -> value or empty
    [ -f "$1" ] || return 0
    awk -v key="$2" '
        NR == 1 && !/^---/ { exit }
        /^---/ { c++; if (c == 2) exit; next }
        c == 1 && index($0, key ":") == 1 { v = substr($0, length(key) + 2); gsub(/^[ \t]+|[ \t]+$/, "", v); print v; exit }
    ' "$1"
}

store_project_slug() {
    # store_project_slug REPO -> registered project slug, or empty.
    [ -f "${PULSE_HOME:-$HOME/.pulse}/pulse.sqlite" ] || return 0
    command -v python3 >/dev/null 2>&1 || return 0
    [ -f "$SCRIPTS/pulse.py" ] || return 0
    python3 - "$SCRIPTS/pulse.py" "$REPO" <<'PY' 2>/dev/null || true
import importlib.util
import pathlib
import sys

spec = importlib.util.spec_from_file_location("pulse_store_for_reflect", sys.argv[1])
pulse = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pulse)
repo = pathlib.Path(sys.argv[2]).resolve()
try:
    con = pulse.open_store(readonly=True)
    try:
        for project in pulse.registry_from_store(con).get("projects", []):
            if pathlib.Path(project.get("path", "")).resolve() == repo:
                print(project.get("slug", ""))
                break
    finally:
        con.close()
except Exception:
    pass
PY
}

product_scope() {
    # product_scope CONSTITUTION EVALUATORS -> prints the PRODUCT SCOPE section;
    # returns 0 when the loop may improve the product, 1 when it is confined to
    # the operating layer. Read-only.
    local const="$1" evalf="$2" classes_ok=1 evals_ok=1
    echo "PRODUCT SCOPE (the loop's subject is the product the repository ships)"
    if [ ! -f "$const" ]; then
        echo "  no constitution"
        echo ""
        return 1
    fi
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
        }' "$const"; then classes_ok=0; fi
    if [ -f "$evalf" ] && grep -qiE '^##+[ \t]+Product evaluators' "$evalf"; then
        echo "  product evaluators: present"
        evals_ok=0
    else
        echo "  product evaluators: absent - add a '## Product evaluators' section to evaluators.md (the project's verification as a constraint, at least one product score) and a baseline row"
    fi
    if [ "$classes_ok" -eq 0 ] && [ "$evals_ok" -eq 0 ]; then
        echo "  verdict: the loop can improve the product"
        echo ""
        return 0
    fi
    echo "  verdict: the loop is confined to the operating layer (docs) until the lines above are fixed"
    echo ""
    return 1
}

hygiene() {
    # hygiene REPO -> prints stray worktrees and evolve/* branches. Read-only.
    local repo="$1" extra br rbr
    echo "HYGIENE (branches, worktrees, and clones the loop must remove - docs/workflows/evolve.md, Clean)"
    if git -C "$repo" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        extra="$(git -C "$repo" worktree list --porcelain 2>/dev/null | awk '/^worktree / { n++; if (n > 1) print "    " substr($0, 10) }')"
        if [ -n "$extra" ]; then
            echo "  worktrees: extra beyond the main tree - remove each when its cycle has ended:"
            printf '%s\n' "$extra"
        else
            echo "  worktrees: main tree only"
        fi
        br="$(git -C "$repo" branch --list 'evolve/*' --format='%(refname:short)' 2>/dev/null)"
        if [ -n "$br" ]; then
            echo "  evolve/* branches: $(printf '%s\n' "$br" | wc -l | tr -d ' ') - delete each when its cycle ended or its pull request closed:"
            printf '%s\n' "$br" | sed 's/^/    /'
        else
            echo "  evolve/* branches: none"
        fi
        rbr="$(git -C "$repo" branch -r --list '*/evolve/*' --format='%(refname:short)' 2>/dev/null)"
        if [ -n "$rbr" ]; then
            echo "  remote evolve/* branches: $(printf '%s\n' "$rbr" | wc -l | tr -d ' ') - delete on the remote once the pull request merged or closed:"
            printf '%s\n' "$rbr" | sed 's/^/    /'
        fi
    else
        echo "  not a git repository"
    fi
    echo ""
}

echo "REFLECT - $REPO - $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo ""

# --- Working state -----------------------------------------------------------
echo "WORKING STATE"
if git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    branch="$(git -C "$REPO" symbolic-ref --short -q HEAD || echo detached)"
    head="$(git -C "$REPO" rev-parse --short HEAD)"
    dirty="$(git -C "$REPO" status --porcelain | wc -l | tr -d ' ')"
    subject="$(git -C "$REPO" log -1 --format=%s)"
    echo "  branch $branch at $head, $dirty uncommitted path(s)"
    echo "  last commit: $subject"
else
    echo "  not a git repository"
fi
echo ""

# --- Board -------------------------------------------------------------------
HAS_BOARD=0
[ -d "$BOARD" ] && HAS_BOARD=1
STORE_SLUG="$(store_project_slug)"
if [ -n "$STORE_SLUG" ] && [ "$SHOW_BOARD" -eq 1 ] && [ -x "$SCRIPTS/pulse.py" ]; then
    echo "BOARD"
    echo "  source: pulse store"
    if [ -n "$AREA" ]; then
        python3 "$SCRIPTS/pulse.py" board --project "$STORE_SLUG" --area "$AREA" | sed 's/^/  /'
    else
        python3 "$SCRIPTS/pulse.py" board --project "$STORE_SLUG" | sed 's/^/  /'
    fi
    echo ""
    # The ballot: what this identity can still vote on. Voting is the one
    # thing every agent can do for the queue before it leaves.
    echo "BALLOT (vote before you leave; one vote per agent, latest wins, no self-votes - docs/workflows/pulse.md)"
    if [ -n "$AGENT" ]; then
        python3 "$SCRIPTS/pulse.py" ballot --project "$STORE_SLUG" --agent "$AGENT" | sed 's/^/  /'
    else
        python3 "$SCRIPTS/pulse.py" ballot --project "$STORE_SLUG" | sed 's/^/  /'
    fi
    echo ""
    # Proposals the agents agreed on that wait for the human: the closing
    # message repeats each one in plain words and asks for a yes or no
    # (docs/workflows/board.md, Deliberation). Never re-open a settled debate.
    echo "FOR THE HUMAN (repeat every awaiting proposal in your closing message, one yes/no each; only the human decides - docs/workflows/pulse.md)"
    python3 "$SCRIPTS/pulse.py" proposals --project "$STORE_SLUG" | sed 's/^/  /'
    echo ""
    echo "EXPERIMENT QUEUE"
    python3 "$SCRIPTS/pulse.py" queue --project "$STORE_SLUG" | sed 's/^/  /'
    echo ""
elif [ "$HAS_BOARD" -eq 1 ] && [ "$SHOW_BOARD" -eq 1 ] && [ -x "$SCRIPTS/board.sh" ]; then
    echo "BOARD"
    echo "  source: markdown board"
    if [ -n "$AREA" ]; then
        "$SCRIPTS/board.sh" --repo "$REPO" --area "$AREA" | sed 's/^/  /'
    else
        "$SCRIPTS/board.sh" --repo "$REPO" | sed 's/^/  /'
    fi
    echo ""
    echo "BALLOT (vote before you leave; one vote per identity, latest wins, no self-votes - docs/workflows/board.md step 8)"
    if [ -n "$AGENT" ]; then
        "$SCRIPTS/board.sh" --repo "$REPO" --ballot --agent "$AGENT" | sed 's/^/  /'
    else
        "$SCRIPTS/board.sh" --repo "$REPO" --ballot | sed 's/^/  /'
    fi
    echo ""
    echo "FOR THE HUMAN (repeat every awaiting proposal in your closing message, one yes/no each; record the answer as a result post - docs/workflows/board.md)"
    "$SCRIPTS/board.sh" --repo "$REPO" --proposals --no-notices | sed '1d;/^$/d' | sed 's/^/  /'
    echo ""
elif [ "$HAS_BOARD" -eq 0 ]; then
    echo "BOARD"
    echo "  none (docs/board/ absent; Level B is not available)"
    echo ""
fi

# --- Commons (ADR-016): suggestions about PULSE itself from every project ----
# The framework repository (the brain) carries eligible commons suggestions to
# its own board; every other project sees the open questions it could answer.
if [ "$SHOW_BOARD" -eq 1 ] && [ -x "$SCRIPTS/commons.sh" ]; then
    COMMONS_DIR="$("$SCRIPTS/commons.sh" --path 2>/dev/null || true)"
    echo "COMMONS (docs/workflows/commons.md; suggestions here are signals for the brain, never experiments)"
    if [ -n "$COMMONS_DIR" ] && [ -d "$COMMONS_DIR/docs/board" ]; then
        "$SCRIPTS/commons.sh" --queue 2>/dev/null | sed 's/^/  /'
        "$SCRIPTS/commons.sh" --area questions --threads 2>/dev/null | sed 's/^/  /'
    else
        echo "  not created (owner: docs/scripts/commons.sh --init)"
    fi
    [ -f "$SCRIPTS/nudge.py" ] && echo "  every project at once: python3 docs/scripts/nudge.py --agent <runner>/<model> (read-only work digest - docs/workflows/nudge.md)"
    echo ""
fi

# --- Loop gates --------------------------------------------------------------
echo "LOOP GATES"
HAS_EVO=0; STOP=0; LOOP=""; LIMITS=""; BASELINE=""; BASELINE_ROW=0; HELD_OUT=""; BUDGET=""; USED_TODAY=0; FILLED=0
if [ -d "$EVO" ]; then
    HAS_EVO=1
    [ -e "$EVO/STOP" ] && STOP=1
    LOOP="$(frontmatter_value "$EVO/constitution.md" loop)"
    LIMITS="$(frontmatter_value "$EVO/constitution.md" limits | tr '[:upper:]' '[:lower:]')"
    # absent means the Budget table binds; "none" means nothing in it does
    [ -z "$LIMITS" ] && [ -f "$EVO/constitution.md" ] && LIMITS="budget"
    BASELINE="$(frontmatter_value "$EVO/evaluators.md" baseline)"
    if [ -f "$EVO/evaluators.md" ]; then
        HELD_OUT="$(awk '/^Held out:/ { sub(/^Held out:[ \t]*/, ""); print; exit }' "$EVO/evaluators.md")"
    fi
    if [ -n "$BASELINE" ] && [ "$BASELINE" != "pending" ] && [ -f "$EVO/archive.md" ] \
        && grep -q "^| $BASELINE " "$EVO/archive.md"; then
        BASELINE_ROW=1
    fi
    if [ -f "$EVO/constitution.md" ] && [ "$LIMITS" != "none" ]; then
        BUDGET="$(awk -F'|' 'tolower($2) ~ /cycles per day/ { gsub(/[ \t]/, "", $3); print $3; exit }' "$EVO/constitution.md")"
        # anything that is not a plain number (unlimited, continuous, blank) means no cap
        case "$BUDGET" in ''|*[!0-9]*) BUDGET="" ;; esac
    fi
    if [ -f "$EVO/archive.md" ]; then
        USED_TODAY="$(awk -F'|' -v today="$TODAY" '$2 ~ /^ *A-[0-9]+ *$/ { gsub(/[ \t]/, "", $3); if ($3 == today) n++ } END { print n + 0 }' "$EVO/archive.md")"
    fi
    if [ -f "$EVO/constitution.md" ] && [ -f "$EVO/evaluators.md" ] && [ -f "$EVO/archive.md" ] && [ -n "$LOOP" ]; then
        FILLED=1
    fi
    [ "$STOP" -eq 1 ] && echo "  KILL SWITCH: docs/evolution/STOP present" || echo "  kill switch: not engaged"
    echo "  constitution: $([ -f "$EVO/constitution.md" ] && echo "present, loop: ${LOOP:-unset}" || echo missing)"
    echo "  evaluators:   $([ -f "$EVO/evaluators.md" ] && echo "present, baseline: ${BASELINE:-unset}$([ "$BASELINE_ROW" -eq 1 ] && echo ' (row found)' || echo ' (no archive row)'), held out: ${HELD_OUT:-unset}" || echo missing)"
    echo "  archive:      $([ -f "$EVO/archive.md" ] && echo present || echo missing)"
    if [ "$LIMITS" = "none" ]; then
        echo "  limits:       none - no budget limit binds (tokens, wall time, cycles, plateau); only stop rules end the run"
        echo "  cycle cap:    none ($USED_TODAY cycle(s) today)"
    elif [ -n "$BUDGET" ]; then
        echo "  limits:       budget - the Budget table binds"
        echo "  cycle cap:    $USED_TODAY of $BUDGET cycle(s) used today"
    else
        echo "  limits:       ${LIMITS:-budget} - the Budget table binds; Cycles per day is not a number, so no cap"
        echo "  cycle cap:    none - continuous mode ($USED_TODAY cycle(s) today)"
    fi
else
    echo "  none (docs/evolution/ absent; Levels C and D are not available)"
fi
echo ""

# --- Product scope and hygiene ----------------------------------------------
PRODUCT_OK=1
if [ "$HAS_EVO" -eq 1 ]; then
    if product_scope "$EVO/constitution.md" "$EVO/evaluators.md"; then PRODUCT_OK=0; fi
fi
hygiene "$REPO"

# --- Health checks -----------------------------------------------------------
echo "HEALTH CHECKS"
if [ -x "$SCRIPTS/check-links.sh" ]; then
    if out="$("$SCRIPTS/check-links.sh" --repo "$REPO" --quiet 2>&1)"; then
        echo "  links:      ok"
    else
        echo "  links:      FAILED - $(printf '%s\n' "$out" | tail -1)"
    fi
fi
bad=0
for s in "$SCRIPTS"/*.sh; do
    [ -f "$s" ] || continue
    bash -n "$s" 2>/dev/null || { bad=$((bad + 1)); echo "  bash -n:    FAILED - $s"; }
done
[ "$bad" -eq 0 ] && echo "  bash -n:    ok"
if git -C "$REPO" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if git -C "$REPO" diff --check >/dev/null 2>&1 && git -C "$REPO" diff --cached --check >/dev/null 2>&1; then
        echo "  whitespace: ok"
    else
        echo "  whitespace: FAILED - git diff --check"
    fi
fi
if [ -x "$SCRIPTS/lead-time.sh" ]; then
    if lt="$("$SCRIPTS/lead-time.sh" --repo "$REPO" 2>&1)"; then
        echo "  lead time:  ok"
        printf '%s\n' "$lt" | awk '/^  Tier |^  Retired|^  Closed without|^  Missing/ { print "   " $0 }'
    else
        echo "  lead time:  FAILED - $(printf '%s\n' "$lt" | tail -1)"
    fi
fi
echo ""

# --- Level -------------------------------------------------------------------
LEVEL="A"
REASON_B=""; REASON_C=""; REASON_D=""
if [ "$HAS_BOARD" -eq 1 ]; then LEVEL="B"; else REASON_B="docs/board/ absent"; fi
if [ "$LEVEL" = "B" ]; then
    if [ "$HAS_EVO" -eq 0 ]; then REASON_C="docs/evolution/ absent"
    elif [ "$STOP" -eq 1 ]; then REASON_C="STOP present"
    elif [ "$FILLED" -eq 0 ]; then REASON_C="constitution, evaluators, or archive missing or unfilled"
    elif [ "$BASELINE_ROW" -eq 0 ]; then REASON_C="no recorded baseline row (baseline: ${BASELINE:-unset})"
    else LEVEL="C"; fi
fi
if [ "$LEVEL" = "C" ]; then
    if [ "$LOOP" != "on" ]; then REASON_D="loop: ${LOOP:-unset}"
    elif [ -z "$HELD_OUT" ] || [ "$HELD_OUT" = "pending" ]; then REASON_D="held-out set pending"
    elif [ -n "$BUDGET" ] && [ "$USED_TODAY" -ge "$BUDGET" ]; then REASON_D="cycle cap reached today ($USED_TODAY of $BUDGET)"
    else LEVEL="D"; fi
fi

echo "LEVEL ALLOWED: $LEVEL"
echo "  A observe   yes - always"
if [ "$HAS_BOARD" -eq 1 ]; then echo "  B record    yes - post signals to the board"; else echo "  B record    no  - $REASON_B"; fi
case "$LEVEL" in
    C|D) echo "  C rehearse  yes - constraint checks, experiment card, optional fleet shadow run" ;;
    *)   echo "  C rehearse  no  - ${REASON_C:-$REASON_B}" ;;
esac
if [ "$LEVEL" = "D" ]; then echo "  D evolve    yes - hand off to pulse-evolve (it re-checks every precondition)"; else echo "  D evolve    no  - ${REASON_D:-${REASON_C:-$REASON_B}}"; fi
echo ""

# --- Next human step ---------------------------------------------------------
echo "NEXT HUMAN STEP"
if [ "$HAS_EVO" -eq 0 ]; then
    echo "  copy docs/evolution/_template-*.md into place and fill the constitution (Stage 1)"
elif [ "$STOP" -eq 1 ]; then
    echo "  the kill switch is engaged; only a human removes docs/evolution/STOP"
elif [ "$FILLED" -eq 0 ]; then
    echo "  fill constitution.md (loop:), evaluators.md, and archive.md from the templates"
elif [ "$BASELINE_ROW" -eq 0 ]; then
    echo "  record the first baseline row in archive.md and set baseline: in evaluators.md"
elif [ -z "$HELD_OUT" ] || [ "$HELD_OUT" = "pending" ]; then
    echo "  run docs/scripts/evolution-setup.sh prepare --store /private/path"
    echo "  fill private H01.md and H02.md, then run the helper's activate command"
elif [ "$LOOP" != "on" ]; then
    echo "  set loop: on in docs/evolution/constitution.md to enter Stage 2"
elif [ -n "$BUDGET" ] && [ "$USED_TODAY" -ge "$BUDGET" ]; then
    echo "  none today; the cap this constitution wrote resets tomorrow"
else
    echo "  none; the loop may run - continuous mode, cycle after cycle, until a stop rule fires"
    [ "$LIMITS" = "none" ] && echo "  (limits: none - no token, wall-time, cycle, or plateau limit binds; never pause to ask, post a question and continue)"
fi
if [ "$HAS_EVO" -eq 1 ] && [ "$FILLED" -eq 1 ] && [ "$PRODUCT_OK" -ne 0 ]; then
    echo "  product: the loop is confined to the operating layer - list the product's paths at stage 4, keep only hard-stop surfaces immutable, and add product evaluators (see PRODUCT SCOPE above); skip only if this repository's product is its docs"
fi
echo ""
echo "SIGNALS TO LIST FROM THIS SESSION (Level A output)"
echo "  product first: failing or flaky tests, type and lint findings, coverage gaps, accessibility"
echo "  or performance reports, TODO/FIXME touched, unmet acceptance criteria, bug reports;"
echo "  then verification failures, rollbacks, review findings, tier redeclarations,"
echo "  requests whose premise was already satisfied, questions answered from scratch,"
echo "  rules that read two ways, artifacts written then discarded, stale fixtures, strays above."
echo "Rules: docs/workflows/reflect.md. Fleets: docs/workflows/fleet.md."

# TEMPLATE drift check: if this repo recorded a template baseline and the
# machine registry registers a brain, compare the recorded upstream commit
# with the brain's current HEAD and report how many commits behind it is.
if [ -f "$REPO/.template-sync" ] && [ -f "${PULSE_HOME:-$HOME/.pulse}/projects.json" ] && command -v python3 >/dev/null 2>&1; then
  brain_path="$(python3 - <<'PY'
import json,os,sys
p = os.path.expanduser(os.environ.get('PULSE_HOME','~/.pulse'))
try:
    with open(os.path.join(p,'projects.json')) as f:
        reg = json.load(f)
    for proj in reg.get('projects', []):
        if isinstance(proj, dict) and proj.get('brain'):
            print(proj.get('path',''))
            break
except Exception:
    pass
PY
)"
  if [ -n "$brain_path" ] && [ -d "$brain_path/.git" ]; then
    recorded="$(python3 - "$REPO/.template-sync" 2>/dev/null <<'PY' || true
import json,sys
try:
    obj = json.load(open(sys.argv[1]))
    print(obj.get('lastSyncedCommit','') or obj.get('lastSynced','') or '')
except Exception:
    pass
PY
)"
    if [ -n "$recorded" ]; then
      rec_short="$(git -C "$brain_path" rev-parse --short "$recorded" 2>/dev/null || true)"
      brain_head="$(git -C "$brain_path" rev-parse --short HEAD 2>/dev/null || true)"
      if [ -n "$rec_short" ] && [ -n "$brain_head" ]; then
        behind="$(git -C "$brain_path" rev-list --count "$recorded".."$brain_head" 2>/dev/null || true)"
        if [ -n "$behind" ] && [ "$behind" != "0" ]; then
          echo ""
          echo "TEMPLATE  upstream ${rec_short} (${behind} commits behind the brain)"
          echo "  run pulse-template-sync"
          echo ""
        fi
      fi
    fi
  fi
fi
