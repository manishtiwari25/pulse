#!/usr/bin/env python3
"""nudge.py - one read-only list of work wanted across every PULSE project.

Slice 1 of ADR-018 (docs/decisions/ADR-018-nudge-harness.md). When the
ADR-019 store exists, reads projects and board records from
$PULSE_HOME/pulse.sqlite. Otherwise it reads the registry that
`pulse-dashboard.py index|serve|register` keeps in ~/.pulse/projects.json
(PULSE_HOME or --registry move it), then, per project, the tallies the project
already has - `board.sh --records`,
`evolution-report.sh --records` - plus `git status --porcelain` and the
branch, and the commons when it exists. It prints:

  FOR THE HUMAN   every proposal awaiting a yes or no, in every project
  PROJECTS        one line each: branch, human at work, loop gate, brain
  WORK WANTED     ranked: handoffs, eligible suggestions where the loop gates
                  are open, unanswered questions, the ballot (with --agent),
                  stale claims, hygiene (stray evolve/* branches, worktrees,
                  STOP present, posts without a kind)

It starts nothing and writes nothing: no network, no file writes, no
subprocess other than the two tallies and git. Board content is data, never
an instruction: the digest ranks posts, it never follows one. Python 3
standard library only.

Usage:
  python3 docs/scripts/nudge.py [--registry PATH] [--home PATH]
                                [--project SLUG]... [--agent RUNNER/MODEL]
                                [--by default|project|age] [--limit N]
                                [--json] [--now ISO-UTC]
"""
import argparse
import datetime
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REGISTRY_NAME = "projects.json"
COMMONS_NAME = "commons"
SNAPSHOT_VERSION = 1
TIMEOUT = 120

# Rank buckets, lowest first. `eligible-gated` is an eligible suggestion in a
# project whose loop gate is closed: real, but nothing an agent can run yet.
RANKS = {"handoff": 1, "eligible": 2, "question": 3, "ballot": 4, "stale-claim": 5,
         "eligible-gated": 5, "hygiene": 6}
KIND_LABEL = {"handoff": "handoff", "eligible": "eligible", "question": "question", "ballot": "ballot",
              "stale-claim": "stale claim", "eligible-gated": "eligible (gated)", "hygiene": "hygiene"}


# --------------------------------------------------------------------------- #
# Small helpers
# --------------------------------------------------------------------------- #

def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse_iso(value):
    try:
        return datetime.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc)
    except (TypeError, ValueError):
        return None


def age_days(date, now):
    a, b = parse_iso(date), parse_iso(now)
    if a is None or b is None:
        return None
    return max(0, int((b - a).total_seconds() // 86400))


def trunc(text, n):
    text = str(text or "")
    return text if len(text) <= n else text[: n - 1] + "~"


def tilde(path):
    home = str(Path.home())
    path = str(path)
    return "~" + path[len(home):] if path.startswith(home + os.sep) or path == home else path


def run(cmd, cwd=None):
    """Run one read-only command; return (returncode, stdout, stderr) and never raise."""
    try:
        out = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=TIMEOUT, check=False)
    except FileNotFoundError:
        return 127, "", "%s not found" % cmd[0]
    except subprocess.TimeoutExpired:
        return 124, "", "%s did not finish within %d seconds" % (cmd[0], TIMEOUT)
    return out.returncode, out.stdout, out.stderr


def git(repo, *args):
    code, out, _ = run(["git", "-C", str(repo)] + list(args))
    return out.strip() if code == 0 else None


def pick_script(repo, name):
    """The project's own tally when it speaks --records; otherwise the copy next to this file."""
    own = Path(repo) / "docs" / "scripts" / name
    if own.is_file():
        try:
            if "--records" in own.read_text(encoding="utf-8", errors="replace"):
                return own
        except OSError:
            pass
    mine = HERE / name
    return mine if mine.is_file() else None


def records(script, args):
    code, out, err = run(["bash", str(script)] + [str(a) for a in args] + ["--records"])
    if code != 0:
        raise RuntimeError("%s exited %d: %s" % (script.name, code, err.strip()[:200]))
    return [line.split("\t") for line in out.split("\n") if line]


def grouped_records(text):
    grouped = {}
    for row in (line.split("\t") for line in text.split("\n") if line):
        grouped.setdefault(field(row, 0), []).append(row)
    return grouped


def field(row, i, default=""):
    return row[i] if len(row) > i else default


# --------------------------------------------------------------------------- #
# Registry and commons (ADR-016), read only
# --------------------------------------------------------------------------- #

def pulse_home(explicit=None):
    if explicit:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get("PULSE_HOME")
    return Path(env).expanduser().resolve() if env else (Path.home() / ".pulse").resolve()


def load_pulse_module():
    spec = importlib.util.spec_from_file_location("pulse_store_for_nudge", HERE / "pulse.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def open_store(home):
    try:
        pulse = load_pulse_module()
        if not pulse.store_exists(home):
            return None, None
        return pulse, pulse.open_store(home, readonly=True)
    except Exception:
        return None, None


def load_registry(path):
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {"projects": [], "commons": ""}, "no registry at %s" % tilde(path)
    except (OSError, ValueError) as exc:
        return {"projects": [], "commons": ""}, "unreadable registry at %s (%s)" % (tilde(path), exc)
    if not isinstance(data, dict) or not isinstance(data.get("projects"), list):
        return {"projects": [], "commons": ""}, "malformed registry at %s" % tilde(path)
    data.setdefault("commons", "")
    return data, None


def commons_path(home, registry):
    env = os.environ.get("PULSE_COMMONS")
    if env:
        return Path(env).expanduser().resolve()
    if registry.get("commons"):
        return Path(registry["commons"]).expanduser().resolve()
    return home / COMMONS_NAME


# --------------------------------------------------------------------------- #
# One project
# --------------------------------------------------------------------------- #

def held_out(repo):
    """The `Held out:` line of evaluators.md, exactly as reflect.sh reads it."""
    path = Path(repo) / "docs" / "evolution" / "evaluators.md"
    try:
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith("Held out:"):
                return line[len("Held out:"):].strip()
    except OSError:
        return ""
    return ""


def loop_gate(repo, evo):
    """Mirror reflect.sh Level D: loop on, STOP absent, baseline row found, held-out listed."""
    if not evo["present"]:
        return {"open": False, "reason": "no docs/evolution"}
    if evo["stop"]:
        return {"open": False, "reason": "STOP present"}
    if evo["loop"] != "on":
        return {"open": False, "reason": "loop: %s" % (evo["loop"] or "unset")}
    if not evo["baseline"] or evo["baseline"] not in evo["baselines"]:
        return {"open": False, "reason": "baseline %s has no archive row" % (evo["baseline"] or "unset")}
    ho = held_out(repo)
    if ho in ("", "pending"):
        return {"open": False, "reason": "held-out set pending"}
    return {"open": True, "reason": "loop on, baseline %s, held-out listed" % evo["baseline"]}


def empty_evolution():
    return {"present": False, "stop": False, "loop": "", "baseline": "", "baselines": set(),
            "branches": [], "worktrees": [], "remote_branches": [], "error": None}


def read_evolution(repo):
    evo = empty_evolution()
    if not (Path(repo) / "docs" / "evolution").is_dir():
        return evo
    evo["present"] = True
    script = pick_script(repo, "evolution-report.sh")
    if script is None:
        evo["error"] = "no evolution-report.sh"
        return evo
    try:
        rows = records(script, ["--repo", repo])
    except RuntimeError as exc:
        evo["error"] = str(exc)
        return evo
    for row in rows:
        kind = field(row, 0)
        if kind == "param":
            name, value = field(row, 1), field(row, 2)
            if name == "stop_present":
                evo["stop"] = value == "1"
            elif name == "loop":
                evo["loop"] = value
            elif name == "baseline":
                evo["baseline"] = value
        elif kind == "archive_row" and field(row, 1) == "Baselines":
            evo["baselines"].add(field(row, 2))
        elif kind == "hygiene":
            what, value = field(row, 1), field(row, 2)
            if what == "branch":
                evo["branches"].append(value)
            elif what == "worktree":
                evo["worktrees"].append(value)
            elif what == "remote_branch":
                evo["remote_branches"].append(value)
    return evo


def read_board(repo, now):
    """Return the typed records of board.sh --records, grouped by record kind."""
    script = pick_script(repo, "board.sh")
    if script is None:
        raise RuntimeError("no board.sh")
    return grouped_records("\n".join("\t".join(row) for row in records(script, ["--repo", repo, "--now", now])))


def read_board_store(pulse, con, slug, now):
    """Return store records in the board.sh --records layout, grouped by record kind."""
    return grouped_records(pulse.snapshot_records(con, slug, now=now))


def signal(kind, project, key, ident, summary, date, now, nxt, tier, **extra):
    item = {"rank": RANKS[kind], "kind": kind, "project": project, "thread": key, "id": ident,
            "summary": summary, "date": date, "age_days": age_days(date, now), "next": nxt, "tier": tier}
    item.update(extra)
    return item


def project_signals(slug, repo, grouped, evo, gate, agent, now, human_at_work):
    """Turn one project's records into ranked signals. Nothing here reads a post body."""
    out = []
    threads = {field(r, 1): r for r in grouped.get("thread", [])}
    # post record: path area thread id kind agent date expires on vote resolution summary session text_hash read
    P_AREA, P_THREAD, P_ID, P_KIND, P_AGENT, P_DATE, P_SUMMARY = 2, 3, 4, 5, 6, 7, 12
    posts = sorted(grouped.get("post", []), key=lambda r: (field(r, P_DATE), field(r, P_ID)))
    by_thread = {}
    for row in posts:
        by_thread.setdefault(field(row, P_AREA) + "/" + field(row, P_THREAD), []).append(row)

    def post_path(row):
        path = field(row, 1)
        if path.startswith("store:"):
            return "pulse show %s" % field(row, P_ID)
        return "docs/board/%s/%s/%s.md" % (field(row, P_AREA), field(row, P_THREAD), field(row, P_ID))

    def thread_open(key):
        return field(threads.get(key, []), 4) in ("open", "stale")

    # Handoffs: work someone left, in a thread nobody closed or claimed afterwards.
    for key, rows in by_thread.items():
        if not thread_open(key):
            continue
        last_handoff = None
        for row in rows:
            kind = field(row, P_KIND)
            if kind == "handoff":
                last_handoff = row
            elif kind == "claim" and last_handoff is not None:
                last_handoff = None
        if last_handoff is not None:
            out.append(signal("handoff", slug, key, field(last_handoff, P_ID), field(last_handoff, P_SUMMARY), field(last_handoff, P_DATE),
                              now, "read %s, then post a claim and continue the work" % post_path(last_handoff),
                              "as the handoff states", agent=field(last_handoff, P_AGENT), human_at_work=human_at_work))
    # Eligible suggestions: the queue, gated by the loop.
    for row in grouped.get("suggestion", []):
        if field(row, 8) != "eligible":
            continue
        key, ident, summary, date = field(row, 4), field(row, 3), field(row, 10), field(row, 2)
        if evo["stop"]:
            continue  # loop signals are hidden while STOP is present; one hygiene line says so
        if gate["open"]:
            out.append(signal("eligible", slug, key, ident, summary, date, now,
                              "pulse-evolve in this project (docs/workflows/evolve.md); the queue's top row is %s" % ident,
                              "Tier 0-1 (the loop's own limit)", score=int(field(row, 1, "0") or 0), agent=field(row, 9),
                              human_at_work=human_at_work))
        else:
            out.append(signal("eligible-gated", slug, key, ident, summary, date, now,
                              "wait: %s - the owner opens the gate (docs/scripts/reflect.sh names the step)" % gate["reason"],
                              "Tier 0-1 once the gate opens", score=int(field(row, 1, "0") or 0), agent=field(row, 9),
                              human_at_work=human_at_work))
    # Unanswered questions: a question with no answer after it, in an open thread.
    for key, rows in by_thread.items():
        if not thread_open(key):
            continue
        last_q = None
        for row in rows:
            kind = field(row, P_KIND)
            if kind == "question":
                last_q = row
            elif kind == "answer" and last_q is not None:
                last_q = None
        if last_q is not None:
            if field(last_q, 1).startswith("store:"):
                nxt = "answer it: pulse post answer --project %s --area %s --thread %s --on %s --summary '...'" % (
                    slug, field(last_q, P_AREA), field(last_q, P_THREAD), field(last_q, P_ID))
            else:
                nxt = "answer it: post kind answer with on: %s under docs/board/%s/" % (field(last_q, P_ID), key)
            out.append(signal("question", slug, key, field(last_q, P_ID), field(last_q, P_SUMMARY), field(last_q, P_DATE), now,
                              nxt, "Tier 0 (an answer is a post)", agent=field(last_q, P_AGENT), human_at_work=human_at_work))
    # Ballot: what this identity has not voted on and did not write (board.sh --ballot, computed from the same records).
    if agent:
        voted = {field(r, 1) for r in grouped.get("svote", []) if field(r, 2) == agent}
        voted |= {field(r, 1) for r in grouped.get("avote", []) if field(r, 3) == agent}
        for row in grouped.get("suggestion", []):
            if field(row, 8) in ("closed", "expired") or field(row, 9) == agent or field(row, 3) in voted:
                continue
            vote_next = "vote with a one-line reason: pulse vote %s up|down --summary '...'" % field(row, 3) \
                if any(field(p, 1).startswith("store:") for p in grouped.get("post", []) if field(p, 4) == field(row, 3)) \
                else "vote with a one-line reason: docs/scripts/board.sh --ballot --agent %s" % agent
            out.append(signal("ballot", slug, field(row, 4), field(row, 3), field(row, 10), field(row, 2), now,
                              vote_next, "Tier 0 (a vote is a post)", status=field(row, 8), agent=field(row, 9),
                              human_at_work=human_at_work))
        for row in grouped.get("amendment", []):
            if field(row, 4).startswith("closed") or field(row, 16) == agent or field(row, 2) in voted:
                continue
            out.append(signal("ballot", slug, field(row, 3), field(row, 2), field(row, 17), field(row, 1), now,
                              "read the amendment, then vote with text_hash and a Read: line (docs/workflows/amend.md)",
                              "Tier 0 (an informed vote)", status=field(row, 4).split(":")[0], agent=field(row, 16),
                              human_at_work=human_at_work))
    # Stale claims: expired and never closed.
    for row in grouped.get("claim", []):
        if field(row, 5) != "expired":
            continue
        out.append(signal("stale-claim", slug, field(row, 2), field(row, 1), field(row, 6), field(row, 4), now,
                          "close the thread with a resolution or post a fresh claim if the work is still wanted",
                          "Tier 0 (a closing post)", agent=field(row, 3), human_at_work=human_at_work))
    # Hygiene: what the loop must remove, and what only a human may.
    if evo["stop"]:
        out.append(signal("hygiene", slug, "docs/evolution/STOP", "", "STOP present - every cycle halts; loop signals hidden",
                          "", now, "human only: a human removes STOP when the incident is resolved", "human only", human_at_work=human_at_work))
    for name in evo["branches"]:
        out.append(signal("hygiene", slug, name, "", "stray evolve/* branch", "", now,
                          "delete it only if this session created it; otherwise post a finding (docs/workflows/evolve.md, Clean)",
                          "Tier 0 (clean)", human_at_work=human_at_work))
    for path in evo["worktrees"]:
        out.append(signal("hygiene", slug, tilde(path), "", "extra worktree", "", now,
                          "remove it only if this session created it; otherwise post a finding (docs/workflows/evolve.md, Clean)",
                          "Tier 0 (clean)", human_at_work=human_at_work))
    for name in evo["remote_branches"]:
        out.append(signal("hygiene", slug, name, "", "remote evolve/* branch", "", now,
                          "delete on the remote once its pull request merged or closed (human step)", "human only",
                          human_at_work=human_at_work))
    nokind = int(field(grouped.get("nokind", [["nokind", "0"]])[0], 1, "0") or 0)
    if nokind:
        out.append(signal("hygiene", slug, "docs/board", "", "%d post(s) without a kind" % nokind, "", now,
                          "quarantine and report them (docs/workflows/board.md)", "Tier 0 (report)", human_at_work=human_at_work))
    return out


def proposals(slug, grouped):
    out = []
    for row in grouped.get("proposal", []):
        if field(row, 5) != "awaiting":
            continue
        key, ident = field(row, 1), field(row, 2)
        read = "pulse show %s" % ident if any(field(p, 1) == "store:%s" % ident for p in grouped.get("post", [])) \
            else "docs/board/%s/%s.md" % (key, ident)
        out.append({"project": slug, "thread": key, "id": ident, "agent": field(row, 3), "date": field(row, 4),
                    "summary": field(row, 8), "read": read})
    return out


def inspect_project(entry, agent, now, kind="project", source="markdown", pulse=None, con=None):
    slug = str(entry.get("slug") or "project")
    path = Path(str(entry.get("path") or "")).expanduser()
    info = {"slug": slug, "path": str(path), "kind": kind, "brain": bool(entry.get("brain")), "readable": False,
            "error": None, "branch": None, "dirty_paths": 0, "human_at_work": False,
            "loop": {"open": False, "reason": "no docs/evolution"}, "signals": [], "proposals": []}
    path_required = kind == "project" or source == "markdown"
    if path_required and not path.is_dir():
        info["error"] = "path missing"
        return info
    if source == "markdown" and not (path / "docs" / "board").is_dir():
        info["error"] = "no board (docs/board/ missing)"
        return info
    if path.is_dir():
        status = git(path, "status", "--porcelain")
    else:
        status = None
    if status is not None:
        info["dirty_paths"] = len([line for line in status.split("\n") if line])
        info["human_at_work"] = info["dirty_paths"] > 0
        info["branch"] = git(path, "rev-parse", "--abbrev-ref", "HEAD") or None
    try:
        grouped = read_board_store(pulse, con, slug, now) if source == "store" else read_board(path, now)
    except RuntimeError as exc:
        info["error"] = "board tally failed: %s" % exc
        return info
    except Exception as exc:
        info["error"] = "store tally failed: %s" % exc
        return info
    # The commons has no loop of its own (docs/workflows/commons.md).
    evo = read_evolution(path) if kind == "project" and path.is_dir() else empty_evolution()
    if evo["error"]:
        info["loop"] = {"open": False, "reason": evo["error"]}
    else:
        info["loop"] = loop_gate(path, evo)
    info["readable"] = True
    info["proposals"] = proposals(slug, grouped)
    info["signals"] = project_signals(slug, path, grouped, evo, info["loop"], agent, now, info["human_at_work"])
    return info


# --------------------------------------------------------------------------- #
# The digest
# --------------------------------------------------------------------------- #

def order(items, by):
    def age_key(item):
        return -(item["age_days"] if item["age_days"] is not None else -1)

    if by == "project":
        return sorted(items, key=lambda i: (i["project"], i["rank"], -i.get("score", 0), age_key(i), i["id"]))
    if by == "age":
        return sorted(items, key=lambda i: (age_key(i), i["rank"], i["project"], i["id"]))
    return sorted(items, key=lambda i: (i["rank"], -i.get("score", 0), age_key(i), i["project"], i["id"]))


def build(args):
    now = args.now or utc_now()
    home = pulse_home(args.home)
    registry_path = Path(args.registry).expanduser().resolve() if args.registry else home / REGISTRY_NAME
    pulse, con = open_store(home)
    source = "store" if con is not None else "markdown"
    if source == "store":
        registry, reg_error = pulse.registry_from_store(con), None
    else:
        registry, reg_error = load_registry(registry_path)
    wanted = set(args.project or [])
    entries = [e for e in registry["projects"] if isinstance(e, dict)]
    unknown = sorted(s for s in wanted if s != "commons" and s not in {str(e.get("slug")) for e in entries})
    if wanted:
        entries = [e for e in entries if str(e.get("slug")) in wanted]
    project_entries = [e for e in entries if str(e.get("slug")) != "commons"]
    projects = [inspect_project(e, args.agent, now, source=source, pulse=pulse, con=con) for e in project_entries]
    seen = {}
    for p in projects:
        seen.setdefault(p["slug"], []).append(p)
    for slug, same in seen.items():
        if len(same) > 1:
            for n, p in enumerate(same, 1):
                p["duplicate"] = n
    commons = None
    cpath = commons_path(home, registry)
    commons_registered = False
    if source == "store":
        commons_registered = any(str(e.get("slug")) == "commons" for e in registry["projects"] if isinstance(e, dict))
        if not commons_registered:
            try:
                commons_registered = con.execute("SELECT 1 FROM posts WHERE project = ? LIMIT 1", ("commons",)).fetchone() is not None
            except Exception:
                commons_registered = False
    if source == "store" and commons_registered and (not wanted or "commons" in wanted):
        commons_entry = next((e for e in entries if str(e.get("slug")) == "commons"), {"slug": "commons", "path": str(cpath)})
        commons = inspect_project(commons_entry, args.agent, now, kind="commons", source=source, pulse=pulse, con=con)
    elif (cpath / "docs" / "board").is_dir() and (not wanted or "commons" in wanted):
        commons = inspect_project({"slug": "commons", "path": str(cpath)}, args.agent, now, kind="commons")
    if commons and not commons["readable"]:
        commons = None
    if con is not None:
        con.close()
        con = None
        pulse = None
    if wanted and "commons" in wanted and commons is None and source == "store":
        unknown = sorted(set(unknown + ["commons"]))
    if commons:
        # The commons has no loop and no claims of its own; keep questions and the ballot only.
        commons["signals"] = [s for s in commons["signals"] if s["kind"] in ("question", "ballot")]
        commons["loop"] = {"open": False, "reason": "the commons has no loop"}
    work = [s for p in projects for s in p["signals"]] + (commons["signals"] if commons else [])
    work = order(work, args.by)
    for_human = [pr for p in projects for pr in p["proposals"]]
    return {
        "version": SNAPSHOT_VERSION, "source": source, "now": now, "registry": str(registry_path), "registry_error": reg_error,
        "unknown_projects": unknown, "commons": str(cpath) if commons else None,
        "projects": projects + ([commons] if commons else []),
        "for_the_human": sorted(for_human, key=lambda p: (p["date"], p["project"])),
        "work": work,
    }


def to_json(snapshot):
    def clean(obj):
        if isinstance(obj, dict):
            return {k: clean(v) for k, v in obj.items()}
        if isinstance(obj, (list, tuple, set)):
            return [clean(v) for v in sorted(obj, key=str)] if isinstance(obj, set) else [clean(v) for v in obj]
        return obj
    return json.dumps(clean(snapshot), indent=2, sort_keys=True)


def render(snapshot, limit):
    out = []
    projects = [p for p in snapshot["projects"] if p["kind"] == "project"]
    out.append("NUDGE - work wanted across %d project%s - %s" % (len(projects), "" if len(projects) == 1 else "s", snapshot["now"]))
    out.append("  source: %s" % snapshot["source"])
    out.append("  registry: %s%s" % (tilde(snapshot["registry"]), "  (%s)" % snapshot["registry_error"] if snapshot["registry_error"] else ""))
    for slug in snapshot["unknown_projects"]:
        out.append("  unknown project: %s (not in the registry)" % slug)
    out.append("")
    out.append("FOR THE HUMAN (proposals awaiting a yes or no; answer in chat, an agent records it - docs/workflows/board.md)")
    if snapshot["for_the_human"]:
        for pr in snapshot["for_the_human"]:
            out.append("  %-18s %-38s %s" % (trunc(pr["project"], 18), trunc(pr["thread"], 38), trunc(pr["summary"], 70)))
            out.append("  %-18s read %s" % ("", pr["read"]))
    else:
        out.append("  none")
    out.append("")
    out.append("PROJECTS (branch, human at work, loop gate - docs/workflows/nudge.md)")
    if not snapshot["projects"]:
        out.append("  no projects registered - python3 docs/scripts/pulse-dashboard.py register --repo PATH")
    for p in snapshot["projects"]:
        name = p["slug"] + (" (%d)" % p["duplicate"] if p.get("duplicate") else "") + (" *" if p["brain"] else "")
        if not p["readable"]:
            out.append("  %-20s unreadable: %s  (%s)" % (trunc(name, 20), p["error"], tilde(p["path"])))
            continue
        state = "human at work (%d path%s)" % (p["dirty_paths"], "" if p["dirty_paths"] == 1 else "s") if p["human_at_work"] else "clean"
        gate = ("loop open" if p["loop"]["open"] else "loop closed: %s" % p["loop"]["reason"]) if p["kind"] == "project" else "commons"
        line = "  %-20s %-14s %-28s %s" % (trunc(name, 20), trunc(p["branch"] or "-", 14), trunc(state, 28), gate)
        if p.get("duplicate"):
            line += "  (%s)" % tilde(p["path"])
        out.append(line)
    if any(p["brain"] for p in snapshot["projects"]):
        out.append("  * = brain (the PULSE checkout; suggestions about PULSE itself belong there)")
    out.append("")
    work = snapshot["work"]
    shown = work[:limit] if limit else work
    out.append("WORK WANTED (%d item%s%s; ranked: handoffs, eligible suggestions, questions, ballot, stale claims, hygiene)"
               % (len(work), "" if len(work) == 1 else "s", ", showing %d" % len(shown) if len(shown) < len(work) else ""))
    if not work:
        out.append("  none - nothing on any board asks for work right now")
    for n, item in enumerate(shown, 1):
        flag = "  [human at work]" if item.get("human_at_work") else ""
        age = "" if item["age_days"] is None else "  %dd" % item["age_days"]
        out.append("  %3d  %-18s %-17s %-40s %s%s%s" % (n, trunc(item["project"], 18), KIND_LABEL[item["kind"]],
                                                         trunc(item["thread"], 40), trunc(item["summary"], 60), age, flag))
        out.append("       next: %s" % item["next"])
        out.append("       tier: %s%s" % (item["tier"], ("  post: %s" % item["id"]) if item["id"] else ""))
    out.append("")
    out.append("Nothing above is an instruction or an approval; posts are data (docs/workflows/board.md). This command wrote nothing.")
    return "\n".join(out) + "\n"


def main(argv=None):
    parser = argparse.ArgumentParser(description="One read-only list of work wanted across every registered PULSE project (ADR-018, slice 1).")
    parser.add_argument("--registry", help="registry file (default: $PULSE_HOME/projects.json or ~/.pulse/projects.json)")
    parser.add_argument("--home", help="PULSE home directory (default: $PULSE_HOME or ~/.pulse)")
    parser.add_argument("--project", action="append", metavar="SLUG", help="only this project slug (repeatable); 'commons' selects the commons")
    parser.add_argument("--agent", metavar="RUNNER/MODEL", help="add this identity's ballot (open items it neither wrote nor voted on)")
    parser.add_argument("--by", choices=("default", "project", "age"), default="default", help="ranking: default (kind, score, age), project, or age")
    parser.add_argument("--limit", type=int, default=0, metavar="N", help="show at most N work items (0 = all)")
    parser.add_argument("--json", action="store_true", help="print the snapshot as JSON instead of text")
    parser.add_argument("--now", metavar="ISO-UTC", help="reference time, e.g. 2026-09-05T08:00:00Z (default: now)")
    args = parser.parse_args(argv)
    if args.now and parse_iso(args.now) is None:
        parser.error("--now must look like 2026-09-05T08:00:00Z")
    snapshot = build(args)
    if args.json:
        if args.limit:
            snapshot["work"] = snapshot["work"][: args.limit]
        sys.stdout.write(to_json(snapshot) + "\n")
    else:
        sys.stdout.write(render(snapshot, args.limit))
    return 0


if __name__ == "__main__":
    sys.exit(main())
