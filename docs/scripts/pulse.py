#!/usr/bin/env python3
"""pulse - the agents' store and runner for PULSE (ADR-019).

One SQLite store per machine ($PULSE_HOME/pulse.sqlite, default
~/.pulse/pulse.sqlite) holds the operating data of every project that adopted
PULSE: projects, agents, posts (the board of every project and the commons),
jobs, runs, the human's decisions, settings, and an append-only event log.
Posts and events are immutable by trigger. Rules and documents (AGENTS.md,
skills, workflows, decisions, plans, memory, the constitution) stay files in
Git; this store is what agents read and write as data.

Standard library only. Run `pulse --help`, or read docs/workflows/pulse.md.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import hashlib
import json
import os
import re
import shlex
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

SCHEMA_VERSION = 1
STORE_NAME = "pulse.sqlite"
COMMONS_SLUG = "commons"
HUMAN_AGENT = "human"
KINDS = ("claim", "handoff", "finding", "question", "answer", "suggestion",
         "opinion", "vote", "amendment", "result")
RESOLUTIONS = ("answered", "promoted", "tested", "ratified", "agreed",
               "accepted", "declined", "dropped", "proceeded")
USEFUL_KINDS = ("finding", "answer", "result", "vote", "amendment", "opinion")
JOB_KINDS = ("onboard", "task", "improve", "answer", "clean", "reflect")
NAME_WORDS = ("aurora", "borealis", "cinder", "delta", "ember", "fjord", "granite",
              "harbor", "iris", "juniper", "kestrel", "lumen", "meridian", "nimbus",
              "orbit", "pallas", "quartz", "ripple", "sable", "tundra", "umber",
              "vesper", "willow", "xenon", "yarrow", "zephyr")
DEFAULT_SETTINGS = {
    "jobs_per_day": "2",
    "minutes_per_job": "10",
    "workers": "1",
    "max_ai_credits": "30",  # the copilot CLI refuses anything below 30
    "prune_after_days": "14",
    "claim_hours": "4",
    "quorum": "2",
    "expiry_days": "14",
    "stale_days": "30",
    "amend_quorum": "40",
    "amend_majority": "2/3",
    "runner_cmd": "copilot",
    "worker_model": "",
    "stop": "0",
}
PROTECTED_DEFAULT = "Hard stops, Kill switch, Amendments, frontmatter"
POST_RECORD_FIELDS = ("path", "area", "thread", "id", "kind", "agent", "date", "expires",
                      "on", "vote", "resolution", "summary", "session", "text_hash", "read")


class PulseError(Exception):
    def __init__(self, message, code=1):
        super().__init__(message)
        self.code = code


# --------------------------------------------------------------------------- #
# Small helpers
# --------------------------------------------------------------------------- #

def utc_now():
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def stamp_of(iso):
    return iso.replace("-", "").replace(":", "")


def parse_iso(value):
    try:
        return _dt.datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=_dt.timezone.utc)
    except (TypeError, ValueError):
        raise PulseError("Not a UTC time like 2026-09-05T08:00:00Z: %r" % (value,), 2)


def day_number(value):
    try:
        return _dt.date.fromisoformat(str(value)[:10]).toordinal()
    except ValueError:
        return 0


def plus_hours(iso, hours):
    return (parse_iso(iso) + _dt.timedelta(hours=float(hours))).strftime("%Y-%m-%dT%H:%M:%SZ")


def slugify(name, limit=40):
    slug = re.sub(r"[^a-z0-9]+", "-", str(name).lower()).strip("-")
    return (slug[:limit] or "x").rstrip("-") or "x"


def sha256_text(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def trunc(text, n):
    text = text or ""
    return text[: n - 1] + "~" if len(text) > n else text


def plural(n, word):
    return word if n == 1 else word + "s"


def is_inside(path, root):
    try:
        Path(path).resolve().relative_to(Path(root).resolve())
        return True
    except ValueError:
        return False


def git(repo, *args, timeout=60, check=False):
    cmd = ["git", "-C", str(repo)] + list(args)
    out = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout, check=False)
    if check and out.returncode != 0:
        raise PulseError("git %s failed in %s: %s" % (" ".join(args), repo, out.stderr.strip()))
    return out


def git_branch(repo):
    try:
        out = git(repo, "rev-parse", "--abbrev-ref", "HEAD", timeout=10)
    except (OSError, subprocess.TimeoutExpired):
        return ""
    return out.stdout.strip() if out.returncode == 0 else ""


def pulse_home(explicit=None):
    if explicit:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get("PULSE_HOME")
    if env:
        return Path(env).expanduser().resolve()
    return (Path.home() / ".pulse").resolve()


def store_path(home=None):
    return pulse_home(home) / STORE_NAME


def split_frontmatter(text):
    lines = text.split("\n")
    if not lines or lines[0].strip() != "---":
        return {}, text
    fm = {}
    for i in range(1, len(lines)):
        if lines[i].strip() == "---":
            return fm, "\n".join(lines[i + 1:])
        m = re.match(r"^([a-z_]+):(.*)$", lines[i])
        if m:
            val = m.group(2).strip()
            if len(val) >= 2 and val[0] == '"' and val[-1] == '"':
                val = val[1:-1]
            fm[m.group(1)] = val
    return fm, ""


def has_read_line(body):
    return 1 if re.search(r"^Read:[ \t]*\S", body or "", re.M) else 0


def fenced_block(body, heading):
    """The fenced block under `## <heading>`, normalized exactly as board.sh does.

    Trailing whitespace stripped from every line, leading and trailing blank
    lines dropped. Returns the list of lines, or None when the heading, the
    fence, or the text is missing.
    """
    state, fence, lines = 0, "", []
    for raw in (body or "").split("\n"):
        if state == 0:
            if re.match(r"^## %s[ \t]*$" % re.escape(heading), raw):
                state = 1
            continue
        if state == 1:
            if raw.startswith("## "):
                break
            m = re.match(r"^(`+)", raw)
            if m:
                fence, state = m.group(1), 2
            continue
        if state == 2:
            if re.match(r"^%s[ \t]*$" % re.escape(fence), raw):
                state = 3
                break
            lines.append(raw.rstrip(" \t"))
    if state < 2:
        return None
    while lines and lines[0] == "":
        lines.pop(0)
    while lines and lines[-1] == "":
        lines.pop()
    return lines or None


def block_hash(lines):
    return sha256_text("\n".join(lines) + "\n")


def table_cell(text, name):
    """The value cell of `| <name> | <value> |` in a Markdown table, or None."""
    m = re.search(r"\|[ \t]*%s[ \t]*\|([^|]*)" % re.escape(name), text)
    if not m:
        return None
    return m.group(1).replace("`", "").strip()


def table_num(text, name):
    cell = table_cell(text, name)
    if cell is None:
        return None
    m = re.search(r"([0-9]+)/([0-9]+)", cell)
    if m:
        return int(m.group(1)) / int(m.group(2))
    m = re.search(r"[0-9]+", cell)
    if m:
        return int(m.group(0))
    return None


# --------------------------------------------------------------------------- #
# The store
# --------------------------------------------------------------------------- #

SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS settings (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    updated_by TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS projects (
    slug TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT NOT NULL UNIQUE,
    remote TEXT NOT NULL DEFAULT '',
    brain INTEGER NOT NULL DEFAULT 0,
    verify_cmd TEXT NOT NULL DEFAULT '',
    default_branch TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'active',
    added_at TEXT NOT NULL,
    added_by TEXT NOT NULL DEFAULT '',
    last_seen TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS agents (
    name TEXT PRIMARY KEY,
    display TEXT NOT NULL DEFAULT '',
    runner TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    kind TEXT NOT NULL DEFAULT 'session',
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL,
    last_active_at TEXT NOT NULL DEFAULT '',
    last_useful_at TEXT NOT NULL DEFAULT '',
    sessions TEXT NOT NULL DEFAULT '[]',
    jobs_done INTEGER NOT NULL DEFAULT 0,
    jobs_failed INTEGER NOT NULL DEFAULT 0,
    posts INTEGER NOT NULL DEFAULT 0,
    pruned_at TEXT NOT NULL DEFAULT '',
    pruned_reason TEXT NOT NULL DEFAULT '',
    legacy_id TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS posts (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    area TEXT NOT NULL,
    thread TEXT NOT NULL,
    kind TEXT NOT NULL,
    agent TEXT NOT NULL,
    runner TEXT NOT NULL DEFAULT '',
    model TEXT NOT NULL DEFAULT '',
    session TEXT NOT NULL DEFAULT '',
    date TEXT NOT NULL,
    branch TEXT NOT NULL DEFAULT '',
    expires TEXT NOT NULL DEFAULT '',
    on_post TEXT NOT NULL DEFAULT '',
    vote TEXT NOT NULL DEFAULT '',
    text_hash TEXT NOT NULL DEFAULT '',
    has_read_line INTEGER NOT NULL DEFAULT 0,
    resolution TEXT NOT NULL DEFAULT '',
    summary TEXT NOT NULL DEFAULT '',
    body TEXT NOT NULL DEFAULT '',
    links TEXT NOT NULL DEFAULT '',
    files TEXT NOT NULL DEFAULT '',
    name TEXT NOT NULL DEFAULT '',
    origin_project TEXT NOT NULL DEFAULT '',
    extra TEXT NOT NULL DEFAULT '{}',
    origin TEXT NOT NULL DEFAULT 'store',
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS posts_thread ON posts (project, area, thread, id);
CREATE INDEX IF NOT EXISTS posts_on ON posts (on_post);
CREATE INDEX IF NOT EXISTS posts_agent ON posts (agent, date);
CREATE TRIGGER IF NOT EXISTS posts_no_update BEFORE UPDATE ON posts
BEGIN SELECT RAISE(ABORT, 'posts are immutable: post again instead'); END;
CREATE TRIGGER IF NOT EXISTS posts_no_delete BEFORE DELETE ON posts
BEGIN SELECT RAISE(ABORT, 'posts are immutable: post again instead'); END;
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    kind TEXT NOT NULL,
    tier INTEGER NOT NULL DEFAULT 1,
    title TEXT NOT NULL,
    brief TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT '',
    priority INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'queued',
    created_at TEXT NOT NULL,
    created_by TEXT NOT NULL DEFAULT '',
    lease_agent TEXT NOT NULL DEFAULT '',
    lease_until TEXT NOT NULL DEFAULT '',
    generation INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    finished_at TEXT NOT NULL DEFAULT '',
    result TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS jobs_status ON jobs (status, priority, created_at);
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    project TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'running',
    exit_code INTEGER,
    seconds REAL,
    clone TEXT NOT NULL DEFAULT '',
    branch TEXT NOT NULL DEFAULT '',
    base TEXT NOT NULL DEFAULT '',
    commits INTEGER NOT NULL DEFAULT 0,
    summary TEXT NOT NULL DEFAULT '',
    log TEXT NOT NULL DEFAULT '',
    verify TEXT NOT NULL DEFAULT '',
    mode TEXT NOT NULL DEFAULT 'real'
);
CREATE TABLE IF NOT EXISTS decisions (
    post_id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    answer TEXT NOT NULL,
    words TEXT NOT NULL DEFAULT '',
    decided_at TEXT NOT NULL,
    decided_by TEXT NOT NULL DEFAULT 'human',
    result_post TEXT NOT NULL DEFAULT ''
);
CREATE TABLE IF NOT EXISTS events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    actor TEXT NOT NULL DEFAULT '',
    action TEXT NOT NULL,
    subject TEXT NOT NULL DEFAULT '',
    detail TEXT NOT NULL DEFAULT ''
);
CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
"""


def store_exists(home=None):
    return store_path(home).is_file()


def connect(home=None, create=False, readonly=False):
    path = store_path(home)
    if not path.is_file() and not create:
        raise PulseError("No store at %s. Run `pulse init` in a project first." % path, 2)
    if create:
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.chmod(path.parent, 0o700)
        except OSError:
            pass
    if readonly:
        con = sqlite3.connect("file:%s?mode=ro" % path, uri=True, timeout=30)
    else:
        con = sqlite3.connect(str(path), timeout=30, isolation_level=None)
    con.row_factory = sqlite3.Row
    con.execute("PRAGMA busy_timeout = 30000")
    con.execute("PRAGMA foreign_keys = ON")
    if not readonly:
        con.execute("PRAGMA journal_mode = WAL")
        ensure_schema(con)
    return con


def ensure_schema(con):
    con.executescript(SCHEMA_SQL)
    row = con.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()
    if row is None:
        now = utc_now()
        con.execute("INSERT INTO meta (key, value) VALUES ('schema_version', ?)", (str(SCHEMA_VERSION),))
        con.execute("INSERT INTO meta (key, value) VALUES ('created_at', ?)", (now,))
        for name, value in DEFAULT_SETTINGS.items():
            con.execute("INSERT OR IGNORE INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'init')",
                        (name, value, now))
        con.execute("INSERT OR IGNORE INTO agents (name, display, kind, status, created_at, notes) VALUES (?, ?, ?, ?, ?, ?)",
                    (HUMAN_AGENT, "the human", "human", "active", now, "the maintainer; records yes or no with `pulse decide`"))
    elif int(row["value"]) > SCHEMA_VERSION:
        raise PulseError("Store schema %s is newer than this tool (%d); update pulse.py" % (row["value"], SCHEMA_VERSION))
    for name, value in DEFAULT_SETTINGS.items():
        con.execute("INSERT OR IGNORE INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, 'init')",
                    (name, value, utc_now()))


def event(con, actor, action, subject="", detail=""):
    if not isinstance(detail, str):
        detail = json.dumps(detail, sort_keys=True)
    con.execute("INSERT INTO events (at, actor, action, subject, detail) VALUES (?, ?, ?, ?, ?)",
                (utc_now(), actor or "", action, subject or "", detail or ""))


def setting(con, name, default=None):
    row = con.execute("SELECT value FROM settings WHERE name = ?", (name,)).fetchone()
    if row is None:
        return DEFAULT_SETTINGS.get(name, default) if default is None else default
    return row["value"]


def setting_int(con, name):
    try:
        return int(setting(con, name))
    except (TypeError, ValueError):
        return int(DEFAULT_SETTINGS[name])


def set_setting(con, name, value, actor):
    if name not in DEFAULT_SETTINGS:
        raise PulseError("Unknown setting %r. Known: %s" % (name, ", ".join(sorted(DEFAULT_SETTINGS))), 2)
    con.execute("INSERT INTO settings (name, value, updated_at, updated_by) VALUES (?, ?, ?, ?) "
                "ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, "
                "updated_by = excluded.updated_by", (name, str(value), utc_now(), actor))
    event(con, actor, "setting", name, str(value))


def majority(value):
    m = re.match(r"^\s*(\d+)\s*/\s*(\d+)\s*$", str(value))
    if not m or int(m.group(2)) == 0:
        raise PulseError("A majority must be a fraction such as 2/3, not %r" % (value,), 2)
    return int(m.group(1)), int(m.group(2))


# --------------------------------------------------------------------------- #
# Projects
# --------------------------------------------------------------------------- #

def project_rows(con, include_inactive=False):
    sql = "SELECT * FROM projects" + ("" if include_inactive else " WHERE status = 'active'") + " ORDER BY slug"
    return [dict(r) for r in con.execute(sql)]


def project_by_slug(con, slug):
    row = con.execute("SELECT * FROM projects WHERE slug = ?", (slug,)).fetchone()
    return dict(row) if row else None


def project_for_path(con, path):
    """The registered project whose checkout contains `path`, or None."""
    target = Path(path).resolve()
    best = None
    for row in con.execute("SELECT * FROM projects"):
        root = Path(row["path"])
        if target == root or is_inside(target, root):
            if best is None or len(str(root)) > len(str(best["path"])):
                best = dict(row)
    return best


def unique_slug(con, base, table="projects", column="slug"):
    slug, n = base, 2
    while con.execute("SELECT 1 FROM %s WHERE %s = ?" % (table, column), (slug,)).fetchone():
        slug, n = "%s-%d" % (base, n), n + 1
    return slug


def add_project(con, path, name=None, remote=None, brain=False, verify_cmd="", actor="", slug=None):
    root = Path(path).expanduser().resolve()
    if not root.is_dir():
        raise PulseError("Not a directory: %s" % root, 2)
    if is_inside(store_path(), root):
        raise PulseError("The store %s must live outside the project %s; set PULSE_HOME elsewhere" % (store_path(), root), 2)
    existing = con.execute("SELECT * FROM projects WHERE path = ?", (str(root),)).fetchone()
    if remote is None:
        out = git(root, "config", "--get", "remote.origin.url", timeout=10)
        remote = out.stdout.strip() if out.returncode == 0 else ""
    default_branch = git_branch(root)
    now = utc_now()
    if existing:
        con.execute("UPDATE projects SET remote = COALESCE(NULLIF(?, ''), remote), brain = CASE WHEN ? THEN 1 ELSE brain END, "
                    "verify_cmd = COALESCE(NULLIF(?, ''), verify_cmd), default_branch = COALESCE(NULLIF(?, ''), default_branch), "
                    "status = 'active', last_seen = ? WHERE slug = ?",
                    (remote or "", 1 if brain else 0, verify_cmd or "", default_branch, now, existing["slug"]))
        slug = existing["slug"]
        created = False
    else:
        base = slugify(slug or name or root.name)
        if base == COMMONS_SLUG:
            base = "%s-project" % base
        slug = unique_slug(con, base)
        con.execute("INSERT INTO projects (slug, name, path, remote, brain, verify_cmd, default_branch, status, added_at, added_by, last_seen) "
                    "VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?, ?, ?)",
                    (slug, name or root.name, str(root), remote or "", 1 if brain else 0, verify_cmd or "", default_branch, now, actor, now))
        created = True
    if brain:
        con.execute("UPDATE projects SET brain = CASE WHEN slug = ? THEN 1 ELSE 0 END", (slug,))
    event(con, actor, "project.add" if created else "project.refresh", slug, str(root))
    return project_by_slug(con, slug), created


def brain_project(con):
    row = con.execute("SELECT * FROM projects WHERE brain = 1 AND status = 'active' LIMIT 1").fetchone()
    return dict(row) if row else None


# --------------------------------------------------------------------------- #
# Agents: unique names, not models
# --------------------------------------------------------------------------- #

def valid_name(name):
    return bool(re.match(r"^[a-z][a-z0-9-]{1,39}$", name or ""))


def agent_row(con, name):
    row = con.execute("SELECT * FROM agents WHERE name = ?", (name,)).fetchone()
    return dict(row) if row else None


def resolve_agent(con, name=None, required=True):
    """The agent this process acts as: --agent, then $PULSE_AGENT (a name or a legacy runner/model id)."""
    ident = name or os.environ.get("PULSE_AGENT", "")
    if not ident:
        if required:
            raise PulseError("Who are you? Pass --agent NAME or set PULSE_AGENT (join first: pulse agent join --name NAME)", 2)
        return None
    row = agent_row(con, ident)
    if row is None and "/" in ident:
        hit = con.execute("SELECT * FROM agents WHERE legacy_id = ? ORDER BY last_active_at DESC LIMIT 1", (ident,)).fetchone()
        row = dict(hit) if hit else None
    if row is None:
        if required:
            raise PulseError("No agent named %r. Join first: pulse agent join --name %s --runner R --model M"
                             % (ident, slugify(ident)), 2)
        return None
    if row["status"] == "pruned":
        raise PulseError("Agent %r was pruned on %s (%s); join with a new name" % (row["name"], row["pruned_at"], row["pruned_reason"]), 2)
    return row


def join_agent(con, name, runner="", model="", session="", display="", kind="session", legacy_id=""):
    name = (name or "").strip().lower()
    if not valid_name(name):
        raise PulseError("A name is 2-40 characters: lowercase letters, digits, dashes, starting with a letter (%r)" % name, 2)
    if name == HUMAN_AGENT:
        raise PulseError("%r is the maintainer; pick another name" % name, 2)
    now = utc_now()
    row = agent_row(con, name)
    if row is None:
        con.execute("INSERT INTO agents (name, display, runner, model, kind, status, created_at, last_active_at, sessions, legacy_id) "
                    "VALUES (?, ?, ?, ?, ?, 'active', ?, ?, ?, ?)",
                    (name, display or name, runner, model, kind, now, now, json.dumps([session] if session else []), legacy_id))
        event(con, name, "agent.join", name, {"runner": runner, "model": model, "session": session})
        return agent_row(con, name), True
    if row["status"] == "pruned":
        raise PulseError("The name %r belongs to an agent pruned on %s; names are never reused" % (name, row["pruned_at"]), 2)
    if (runner and row["runner"] and runner != row["runner"]) or (model and row["model"] and model != row["model"]):
        raise PulseError("The name %r is taken by another agent (%s/%s); pick a unique name" % (name, row["runner"], row["model"]), 2)
    sessions = json.loads(row["sessions"] or "[]")
    if session and session not in sessions:
        sessions.append(session)
    # A name the migration created from Markdown history becomes a live agent
    # the first time its owner joins with it; the record and posts stay.
    promoted = row["kind"] == "legacy" and kind in ("session", "worker")
    con.execute("UPDATE agents SET runner = COALESCE(NULLIF(?, ''), runner), model = COALESCE(NULLIF(?, ''), model), "
                "display = COALESCE(NULLIF(?, ''), display), kind = ?, status = 'active', last_active_at = ?, sessions = ? WHERE name = ?",
                (runner, model, display, kind if promoted else row["kind"], now, json.dumps(sessions[-50:]), name))
    event(con, name, "agent.resume", name, {"session": session, "promoted_from": "legacy" if promoted else ""})
    return agent_row(con, name), False


def spawn_agent(con, runner="", model="", kind="worker"):
    taken = {r["name"] for r in con.execute("SELECT name FROM agents")}
    for word in NAME_WORDS:
        if word not in taken:
            return join_agent(con, word, runner=runner, model=model, kind=kind)[0]
    n = 2
    while True:
        for word in NAME_WORDS:
            cand = "%s-%d" % (word, n)
            if cand not in taken:
                return join_agent(con, cand, runner=runner, model=model, kind=kind)[0]
        n += 1


def touch_agent(con, name, useful=False, when=None):
    when = when or utc_now()
    if useful:
        con.execute("UPDATE agents SET last_active_at = MAX(last_active_at, ?), last_useful_at = MAX(last_useful_at, ?) WHERE name = ?",
                    (when, when, name))
    else:
        con.execute("UPDATE agents SET last_active_at = MAX(last_active_at, ?) WHERE name = ?", (when, name))


def prune_agents(con, now=None, days=None, actor="daemon", dry_run=False):
    now = now or utc_now()
    days = int(days if days is not None else setting_int(con, "prune_after_days"))
    cutoff = (parse_iso(now) - _dt.timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ")
    pruned = []
    for row in con.execute("SELECT * FROM agents WHERE status = 'active' AND kind != 'human' ORDER BY name"):
        useful = row["last_useful_at"] or row["last_active_at"] or row["created_at"]
        if useful >= cutoff:
            continue
        busy = con.execute("SELECT 1 FROM jobs WHERE status = 'leased' AND lease_agent = ?", (row["name"],)).fetchone()
        if busy:
            continue
        reason = "no useful output since %s (limit %d days)" % (useful, days)
        pruned.append((row["name"], reason))
        if not dry_run:
            con.execute("UPDATE agents SET status = 'pruned', pruned_at = ?, pruned_reason = ? WHERE name = ?", (now, reason, row["name"]))
            event(con, actor, "agent.prune", row["name"], reason)
    return pruned


def agent_scorecards(con, include_pruned=True):
    sql = "SELECT * FROM agents" + ("" if include_pruned else " WHERE status != 'pruned'") + " ORDER BY status, last_active_at DESC, name"
    rows = [dict(r) for r in con.execute(sql)]
    for row in rows:
        stats = con.execute(
            "SELECT SUM(kind = 'finding') AS findings, SUM(kind = 'suggestion') AS suggestions, SUM(kind = 'vote') AS votes, "
            "SUM(kind = 'answer') AS answers, SUM(kind = 'result') AS results, COUNT(*) AS n, MAX(date) AS last FROM posts WHERE agent = ?",
            (row["name"],)).fetchone()
        row.update({k: (stats[k] or 0) for k in ("findings", "suggestions", "votes", "answers", "results", "n")})
        row["last_post"] = stats["last"] or ""
    return rows


# --------------------------------------------------------------------------- #
# Posts: immutable rows, one per message
# --------------------------------------------------------------------------- #

def post_row(con, post_id):
    row = con.execute("SELECT * FROM posts WHERE id = ?", (post_id,)).fetchone()
    return dict(row) if row else None


def unique_post_id(con, base):
    pid, n = base, 2
    while con.execute("SELECT 1 FROM posts WHERE id = ?", (pid,)).fetchone():
        pid, n = "%s-%d" % (base, n), n + 1
    return pid


def create_post(con, kind, project, area, thread, summary, body="", agent=None, session="", branch=None,
                on=None, vote="", expires="", resolution="", links="", files="", text_hash="",
                origin_project="", name=None, date=None, extra=None, origin="store", post_id=None,
                runner=None, model=None, check_agent=True):
    """Validate and insert one post. Returns the new row. Never updates anything but agent counters."""
    if kind not in KINDS:
        raise PulseError("Kind must be one of: %s" % ", ".join(KINDS), 2)
    area, thread = slugify(area), slugify(thread)
    if not area or not thread or area == "x" or thread == "x":
        raise PulseError("Both --area and --thread are needed (lowercase words with dashes)", 2)
    summary = " ".join((summary or "").split())
    if not summary:
        raise PulseError("A one-line --summary is required", 2)
    if len(summary) > 240:
        raise PulseError("Keep the summary under 240 characters; put the rest in the body", 2)
    if project != COMMONS_SLUG and project_by_slug(con, project) is None:
        raise PulseError("No project %r; run `pulse projects` or `pulse init`" % project, 2)
    if isinstance(agent, dict):
        arow = agent
    elif check_agent:
        arow = resolve_agent(con, agent)
    else:
        arow = agent_row(con, agent) or {"name": agent, "runner": runner or "", "model": model or "", "sessions": "[]"}
    agent_name = arow["name"]
    if not session:
        sessions = json.loads(arow.get("sessions") or "[]")
        session = os.environ.get("PULSE_SESSION", "") or (sessions[-1] if sessions else "")
    if resolution and resolution not in RESOLUTIONS:
        raise PulseError("Resolution must be one of: %s" % ", ".join(RESOLUTIONS), 2)
    if kind == "vote":
        if not on:
            raise PulseError("A vote needs --on POST_ID (the suggestion or amendment it answers)", 2)
        if vote not in ("up", "down"):
            raise PulseError("A vote needs --vote up or --vote down", 2)
    elif vote:
        raise PulseError("Only a vote carries --vote", 2)
    if on:
        target = post_row(con, on)
        if target is None:
            raise PulseError("No post %r to answer" % on, 2)
        if kind == "vote" and target["agent"] == agent_name and agent_name != HUMAN_AGENT:
            raise PulseError("No self-votes: %s wrote %s" % (agent_name, on), 2)
        if kind == "vote" and target["kind"] not in ("suggestion", "amendment"):
            raise PulseError("Votes answer a suggestion or an amendment, not a %s" % target["kind"], 2)
        if kind == "vote" and target["kind"] == "amendment":
            if not text_hash:
                raise PulseError("A vote on an amendment carries --text-hash (see `pulse hash %s`)" % on, 2)
            if not has_read_line(body):
                raise PulseError("A vote on an amendment carries a `Read:` line restating the change in the body", 2)
    if kind == "amendment":
        cur = fenced_block(body, "Current text")
        new = fenced_block(body, "Proposed text")
        if cur is None or new is None:
            raise PulseError("An amendment body needs `## Current text` and `## Proposed text`, each with a fenced block", 2)
        text_hash = text_hash or block_hash(new)
    if expires:
        parse_iso(expires)
    date = date or utc_now()
    parse_iso(date)
    if branch is None:
        proj = project_by_slug(con, project) if project != COMMONS_SLUG else None
        branch = git_branch(proj["path"]) if proj else ""
    pid = post_id or unique_post_id(con, "%s-%s" % (stamp_of(date), agent_name))
    if name is None:
        name = arow.get("display", "") if arow.get("display", "") != agent_name else ""
    con.execute(
        "INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, "
        "text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (pid, project, area, thread, kind, agent_name, arow.get("runner") or runner or "", arow.get("model") or model or "",
         session or "", date, branch or "", expires or "", on or "", vote or "", text_hash or "", has_read_line(body),
         resolution or "", summary, body or "", links or "", files or "", name or "", origin_project or "",
         json.dumps(extra or {}, sort_keys=True), origin, utc_now()))
    if agent_row(con, agent_name):
        con.execute("UPDATE agents SET posts = posts + 1 WHERE name = ?", (agent_name,))
        touch_agent(con, agent_name, useful=kind in USEFUL_KINDS, when=date)
    event(con, agent_name, "post", pid, {"kind": kind, "project": project, "thread": "%s/%s" % (area, thread)})
    return post_row(con, pid)


def posts_for(con, project, area=None):
    sql = "SELECT * FROM posts WHERE project = ?"
    args = [project]
    if area:
        sql += " AND area = ?"
        args.append(area)
    sql += " ORDER BY area, thread, id"
    return [dict(r) for r in con.execute(sql, args)]


def post_rows(con, project, area=None):
    """Posts of a project as (frontmatter dict, body) pairs, the shape the dashboard reads."""
    out = []
    for row in posts_for(con, project, area):
        out.append((row_frontmatter(row), row["body"]))
    return out


def row_frontmatter(row):
    fm = {"kind": row["kind"], "agent": row["agent"], "runner": row["runner"], "model": row["model"],
          "session": row["session"], "date": row["date"], "branch": row["branch"], "summary": row["summary"]}
    for key, col in (("expires", "expires"), ("on", "on_post"), ("vote", "vote"), ("resolution", "resolution"),
                     ("text_hash", "text_hash"), ("links", "links"), ("files", "files"), ("name", "name"),
                     ("project", "origin_project")):
        if row[col]:
            fm[key] = row[col]
    fm["id"] = row["id"]
    fm["area"] = row["area"]
    fm["thread"] = row["thread"]
    fm["path"] = "store:%s" % row["id"]
    return fm


# --------------------------------------------------------------------------- #
# The tally: one definition of every status, shared by digest, records, ballot
# (a port of docs/scripts/board.sh, which stays as the read-only legacy reader)
# --------------------------------------------------------------------------- #

def table_row(text, name):
    """board.sh `row`: the third cell of the table line whose second cell is exactly `name`."""
    for line in text.split("\n"):
        parts = line.split("|")
        if len(parts) >= 3 and parts[1].strip() == name:
            return parts[2].strip()
    return None


def thresholds(con, project, quorum=None, expiry_days=None, stale_days=None, amend_quorum=None, amend_majority=None):
    """Flags win, then the constitution's Vote rules rows, then settings, then defaults."""
    root = Path(project["path"]) if project and project.get("path") else None
    constitution = root / "docs" / "evolution" / "constitution.md" if root else None
    text = constitution.read_text(encoding="utf-8", errors="replace") if constitution and constitution.is_file() else ""
    protected = ""
    if text:
        if amend_quorum is None:
            cell = table_row(text, "Amendment quorum")
            m = re.search(r"[0-9]+", cell or "")
            amend_quorum = int(m.group(0)) if m else None
        if amend_majority is None:
            cell = table_row(text, "Amendment supermajority")
            m = re.search(r"[0-9]+/[0-9]+", cell or "")
            amend_majority = m.group(0) if m else None
        protected = table_row(text, "Not amendable by vote") or ""
    params = {
        "quorum": int(quorum if quorum is not None else setting_int(con, "quorum")),
        "expiry_days": int(expiry_days if expiry_days is not None else setting_int(con, "expiry_days")),
        "stale_days": int(stale_days if stale_days is not None else setting_int(con, "stale_days")),
        "amend_quorum": int(amend_quorum if amend_quorum is not None else setting_int(con, "amend_quorum")),
        "amend_majority": amend_majority or setting(con, "amend_majority"),
        "protected": protected or PROTECTED_DEFAULT,
        "constitution_present": 1 if text else 0,
        "stop_present": 1 if root and (root / "docs" / "evolution" / "STOP").exists() else 0,
        "constitution": text,
    }
    params["amend_num"], params["amend_den"] = majority(params["amend_majority"])
    return params


def check_amendment(body, constitution, protected):
    """(hash, anchor, section, verdict) exactly as board.sh computes them."""
    prop = fenced_block(body, "Proposed text")
    if prop is None:
        return "", "", "", 'malformed: no fenced block under "## Proposed text"'
    hsh = block_hash(prop)
    cur = fenced_block(body, "Current text")
    if cur is None:
        return hsh, "", "", 'malformed: no fenced block under "## Current text"'
    if not constitution:
        return hsh, "", "", "unchecked: no docs/evolution/constitution.md"
    cur_text = "\n".join(cur) + "\n"
    prop_text = "\n".join(prop) + "\n"
    verdict = "ok"
    cq, pq = table_num(cur_text, "Amendment quorum"), table_num(prop_text, "Amendment quorum")
    if cq is not None and (pq is None or pq < cq):
        verdict = "lowers or removes the amendment quorum"
    cm, pm = table_num(cur_text, "Amendment supermajority"), table_num(prop_text, "Amendment supermajority")
    if verdict == "ok" and cm is not None and (pm is None or pm < cm):
        verdict = "lowers or removes the amendment supermajority"
    for guard in ("Amendment voter", "Informed vote", "Not amendable by vote"):
        if verdict == "ok" and table_cell(cur_text, guard) is not None and table_cell(prop_text, guard) is None:
            verdict = 'removes the "%s" row' % guard
    cp, pp = table_cell(cur_text, "Not amendable by vote"), table_cell(prop_text, "Not amendable by vote")
    if verdict == "ok" and cp is not None and pp is not None:
        for item in cp.split(","):
            it = item.strip()
            if it and ("," + pp + ",").find(it) < 0:
                verdict = 'drops "%s" from the protected sections' % it
    lines = [l.rstrip(" \t") for l in constitution.split("\n")]
    if lines and lines[-1] == "":
        lines.pop()
    hay = "\n" + "\n".join(lines) + "\n"
    needle = "\n" + cur_text
    count, pos, first = 0, 0, -1
    while True:
        k = hay.find(needle, pos)
        if k < 0:
            break
        count += 1
        if first < 0:
            first = k
        pos = k + 1
    sections = []
    if count == 1:
        ln = hay[: first + 1].count("\n")
        span = cur_text.count("\n")
        n = len(lines)
        fm_end = 0
        if n and re.match(r"^---[ \t]*$", lines[0]):
            for i in range(1, n):
                if re.match(r"^---[ \t]*$", lines[i]):
                    fm_end = i + 1
                    break
        sec_of, sec = [], "preamble"
        for i, line in enumerate(lines, start=1):
            if line.startswith("## "):
                sec = line[3:].rstrip(" \t")
            sec_of.append("frontmatter" if 0 < fm_end and i <= fm_end else sec)
        items = [it.strip() for it in protected.replace("`", "").split(",")]
        for j in range(ln, min(ln + span, n + 1)):
            if j < 1 or j > n:
                continue
            s = sec_of[j - 1]
            if s not in sections:
                sections.append(s)
            for it in items:
                if verdict == "ok" and it and it.lower() == s.lower():
                    verdict = 'protected section "%s"' % s
    return hsh, str(count), "|".join(sections), verdict


class Tally:
    """Statuses for the posts of one project (board.sh semantics, one definition each)."""

    def __init__(self, rows, params, now, area=None):
        self.now = now
        self.today = day_number(now)
        self.params = params
        self.all_rows = rows
        self.rows = [r for r in rows if not area or r["area"] == area]
        self.nokind = 0
        self.last, self.lastsum, self.nposts = {}, {}, {}
        self.closed, self.resolution, self.res_id, self.res_date = {}, {}, {}, {}
        self.prop = {}
        self.claims = {}
        self.suggs = {}
        self.votes = {}
        self.order = []
        for r in self.rows:
            key = "%s/%s" % (r["area"], r["thread"])
            if key not in self.nposts:
                self.order.append(key)
            self.nposts[key] = self.nposts.get(key, 0) + 1
            if key not in self.last or r["date"] >= self.last[key]:
                self.last[key], self.lastsum[key] = r["date"], r["summary"]
            if r["resolution"]:
                self.closed[key] = True
                self.resolution[key], self.res_id[key], self.res_date[key] = r["resolution"], r["id"], r["date"]
            if r["resolution"] == "agreed":
                self.prop[key] = {"id": r["id"], "agent": r["agent"], "date": r["date"], "summary": r["summary"]}
            if r["kind"] == "claim":
                self.claims[r["id"]] = {"key": key, "agent": r["agent"], "expires": r["expires"], "summary": r["summary"]}
            if r["kind"] == "suggestion":
                self.suggs[r["id"]] = {"key": key, "agent": r["agent"], "date": r["date"], "summary": r["summary"]}
            if r["kind"] == "vote" and r["on_post"]:
                vk = (r["on_post"], r["agent"])
                if vk not in self.votes or r["date"] >= self.votes[vk]["date"]:
                    self.votes[vk] = {"date": r["date"], "vote": r["vote"], "id": r["id"]}
            if not r["kind"]:
                self.nokind += 1
        # amendments are repository-wide
        self.a_closed, self.a_resolution = {}, {}
        self.amends = {}
        self.avotes = {}
        for r in self.all_rows:
            key = "%s/%s" % (r["area"], r["thread"])
            if r["resolution"]:
                self.a_closed[key] = True
                self.a_resolution[key] = r["resolution"]
            if r["kind"] == "amendment":
                self.amends[r["id"]] = {"key": key, "session": r["session"], "date": r["date"], "summary": r["summary"],
                                        "agent": r["agent"], "body": r.get("body", "")}
            if r["kind"] == "vote" and r["on_post"]:
                vk = (r["on_post"], r["session"])
                if vk not in self.avotes or r["date"] >= self.avotes[vk]["date"]:
                    self.avotes[vk] = {"date": r["date"], "vote": r["vote"], "hash": r["text_hash"],
                                       "read": int(r["has_read_line"] or 0), "agent": r["agent"], "id": r["id"]}

    def claim_status(self, cid):
        c = self.claims[cid]
        if c["key"] in self.closed:
            return "closed"
        if c["expires"] and c["expires"] > self.now:
            return "active"
        if not c["expires"]:
            return "no expiry"
        return "expired"

    def thread_status(self, key):
        if key in self.closed:
            return "closed"
        if self.today - day_number(self.last[key]) > self.params["stale_days"]:
            return "stale"
        return "open"

    def proposal_status(self, key):
        return "awaiting" if self.resolution.get(key) == "agreed" else self.resolution.get(key, "")

    def suggestion_records(self):
        out = []
        for sid, s in self.suggs.items():
            is_closed = s["key"] in self.closed
            up = down = voters = 0
            for (on, agent), v in self.votes.items():
                if on != sid or agent == s["agent"]:
                    continue
                voters += 1
                if v["vote"] == "down":
                    down += 1
                else:
                    up += 1
            score = up - down
            age = self.today - day_number(s["date"])
            if is_closed:
                status = "closed"
            elif voters >= self.params["quorum"]:
                status = "eligible"
            elif voters == 0 and age > self.params["expiry_days"]:
                status = "expired"
            else:
                status = "needs votes"
            out.append({"score": score, "date": s["date"], "id": sid, "key": s["key"], "up": up, "down": down,
                        "voters": voters, "status": status, "agent": s["agent"], "summary": s["summary"]})
        out.sort(key=lambda r: (-r["score"], r["date"], r["id"]))
        return out

    def svote_records(self):
        out = []
        for (on, agent), v in self.votes.items():
            if on not in self.suggs:
                continue
            out.append({"on": on, "agent": agent, "vote_id": v["id"], "date": v["date"], "vote": v["vote"],
                        "class": "self" if agent == self.suggs[on]["agent"] else "counted"})
        return out

    def amendment_records(self):
        p = self.params
        out, avotes = [], []
        for aid, a in self.amends.items():
            hsh, anchor, section, verdict = check_amendment(a["body"], p["constitution"], p["protected"])
            is_closed = a["key"] in self.a_closed
            up = down = voters = selfv = badhash = noread = 0
            idents = set()
            for (on, session), v in self.avotes.items():
                if on != aid:
                    continue
                if session == a["session"] or session == "":
                    selfv += 1
                    cls = "self"
                elif v["hash"] != hsh:
                    badhash += 1
                    cls = "hash mismatch"
                elif v["read"] != 1:
                    noread += 1
                    cls = "no Read line"
                else:
                    voters += 1
                    if v["vote"] == "down":
                        down += 1
                    else:
                        up += 1
                    idents.add(v["agent"])
                    cls = "counted"
                avotes.append({"id": aid, "session": session, "agent": v["agent"], "vote_id": v["id"], "date": v["date"],
                               "vote": v["vote"], "class": cls})
            total = up + down
            pct = int(100 * up / total + 0.5) if total else 0
            if not hsh:
                status = "malformed"
            elif verdict.startswith("malformed") or verdict.startswith("unchecked"):
                status = verdict
            elif verdict != "ok":
                status = "blocked: " + verdict
            elif int(anchor or 0) == 0:
                status = "stale: the replaced text is no longer in the constitution; re-propose"
            elif int(anchor) > 1:
                status = "ambiguous: the replaced text appears %s times; quote more context" % anchor
            elif voters < p["amend_quorum"]:
                status = "needs votes"
            elif up * p["amend_den"] >= p["amend_num"] * total:
                status = "passed; ratification paused (STOP)" if p["stop_present"] else "passed: ratify per docs/workflows/amend.md"
            else:
                status = "short of the supermajority"
            if is_closed:
                status = "closed: " + self.a_resolution[a["key"]]
            out.append({"date": a["date"], "id": aid, "key": a["key"], "status": status, "up": up, "down": down,
                        "voters": voters, "quorum": p["amend_quorum"], "pct": pct, "nid": len(idents), "self": selfv,
                        "badhash": badhash, "noread": noread, "hash": hsh, "section": section, "agent": a["agent"],
                        "summary": a["summary"], "open": not is_closed})
        out.sort(key=lambda r: (r["date"], r["id"]))
        return out, avotes


def project_rows_for_tally(con, project_slug, area=None):
    rows = posts_for(con, project_slug)
    return rows


def records_lines(con, project, area=None, now=None, **flags):
    """The `board.sh --records` layout, computed from the store (paths are store:<id>)."""
    now = now or utc_now()
    slug = project["slug"] if isinstance(project, dict) else project
    proj = project if isinstance(project, dict) else project_by_slug(con, slug)
    params = thresholds(con, proj if slug != COMMONS_SLUG else None, **flags)
    rows = posts_for(con, slug)
    t = Tally(rows, params, now, area)
    L = []
    P = lambda *f: L.append("\t".join(str(x) for x in f))
    P("param", "board_present", 1)
    P("param", "repo", proj["path"] if proj else "store:%s" % slug)
    P("param", "area", area or "")
    P("param", "now", now)
    P("param", "quorum", params["quorum"])
    P("param", "expiry_days", params["expiry_days"])
    P("param", "stale_days", params["stale_days"])
    P("param", "amend_quorum", params["amend_quorum"])
    P("param", "amend_majority", "%d/%d" % (params["amend_num"], params["amend_den"]))
    P("param", "protected", params["protected"])
    P("param", "constitution_present", params["constitution_present"])
    P("param", "stop_present", params["stop_present"])
    for r in rows:
        P("post", "store:%s" % r["id"], r["area"], r["thread"], r["id"], r["kind"], r["agent"], r["date"], r["expires"],
          r["on_post"], r["vote"], r["resolution"], r["summary"].replace("\t", " "), r["session"], r["text_hash"],
          int(r["has_read_line"] or 0))
    P("nokind", t.nokind)
    for cid, c in t.claims.items():
        P("claim", cid, c["key"], c["agent"], c["expires"], t.claim_status(cid), c["summary"])
    for key in t.order:
        P("thread", key, t.nposts[key], t.last[key], t.thread_status(key), t.resolution.get(key, "") if key in t.closed else "",
          t.lastsum[key])
    for key, pr in t.prop.items():
        st = t.proposal_status(key)
        P("proposal", key, pr["id"], pr["agent"], pr["date"], st, "" if st == "awaiting" else t.res_id[key],
          "" if st == "awaiting" else t.res_date[key], pr["summary"])
    for v in t.svote_records():
        P("svote", v["on"], v["agent"], v["vote_id"], v["date"], v["vote"], v["class"])
    for s in t.suggestion_records():
        P("suggestion", s["score"], s["date"], s["id"], s["key"], s["up"], s["down"], s["voters"], s["status"], s["agent"], s["summary"])
    amends, avotes = t.amendment_records()
    for v in avotes:
        P("avote", v["id"], v["session"], v["agent"], v["vote_id"], v["date"], v["vote"], v["class"])
    for a in amends:
        P("amendment", a["date"], a["id"], a["key"], a["status"], a["up"], a["down"], a["voters"], a["quorum"], a["pct"], a["nid"],
          a["self"], a["badhash"], a["noread"], a["hash"], a["section"], a["agent"], a["summary"])
    return L


def snapshot_records(con, project, area=None, now=None, **flags):
    """Records as a string, the shape the dashboard parses (same as `board.sh --records`)."""
    return "\n".join(records_lines(con, project, area, now, **flags)) + "\n"


def tally_for(con, slug, area=None, now=None, **flags):
    now = now or utc_now()
    proj = project_by_slug(con, slug) if slug != COMMONS_SLUG else None
    params = thresholds(con, proj, **flags)
    return Tally(posts_for(con, slug), params, now, area), params


# --------------------------------------------------------------------------- #
# Digest, ballot, proposals: what an agent reads at the start of work
# --------------------------------------------------------------------------- #

def ballot_items(con, slug, agent=None, now=None):
    t, params = tally_for(con, slug, None, now)
    suggestions, amendments = [], []
    voted = set()
    for v in t.svote_records():
        if agent and v["agent"] == agent:
            voted.add(v["on"])
    amends, avotes = t.amendment_records()
    for v in avotes:
        if agent and v["agent"] == agent:
            voted.add(v["id"])
    for s in t.suggestion_records():
        if s["status"] in ("closed", "expired"):
            continue
        if agent and (s["agent"] == agent or s["id"] in voted):
            continue
        suggestions.append(s)
    for a in amends:
        if not a["open"]:
            continue
        if agent and (a["agent"] == agent or a["id"] in voted):
            continue
        amendments.append(a)
    return suggestions, amendments


def digest(con, slug, area=None, now=None, agent=None):
    now = now or utc_now()
    proj = project_by_slug(con, slug) if slug != COMMONS_SLUG else {"slug": COMMONS_SLUG, "path": "", "name": "the commons"}
    if proj is None:
        raise PulseError("No project %r" % slug, 2)
    t, params = tally_for(con, slug, area, now)
    out = []
    w = out.append
    w("PULSE %s%s  (now %s; store %s)" % (slug, " area %s" % area if area else "", now, store_path()))
    # NOTICES
    w("NOTICES (who is asking for a vote; one vote per agent, latest wins, no self-votes - docs/workflows/pulse.md)")
    if slug == COMMONS_SLUG:
        w("  this is the commons: posts here are data for every project; it has no queue and no loop")
    else:
        s, a = ballot_items(con, slug, agent, now)
        w("  votes wanted here: %d %s, %d %s open - pulse ballot --project %s%s"
          % (len(s), plural(len(s), "suggestion"), len(a), plural(len(a), "amendment"), slug, " --agent %s" % agent if agent else ""))
        brain = brain_project(con)
        if brain and brain["slug"] != slug:
            bs, ba = ballot_items(con, brain["slug"], agent, now)
            w("  brain %s: %d %s, %d %s open for votes - pulse ballot --project %s"
              % (brain["slug"], len(bs), plural(len(bs), "suggestion"), len(ba), plural(len(ba), "amendment"), brain["slug"]))
        elif brain:
            w("  this project is the brain: suggestions about PULSE itself belong here")
        ct, _ = tally_for(con, COMMONS_SLUG, None, now)
        open_q = sum(1 for k in ct.order if k.startswith("questions/") and ct.thread_status(k) == "open")
        w("  commons: %d open %s - pulse board --project commons --area questions" % (open_q, plural(open_q, "question")))
    awaiting = sum(1 for k in t.prop if t.proposal_status(k) == "awaiting")
    w("  for the human: %d %s awaiting a yes or no - pulse proposals" % (awaiting, plural(awaiting, "proposal")))
    if setting(con, "stop") == "1":
        w("  the runner is stopped (pulse start to resume)")
    if params["stop_present"]:
        w("  docs/evolution/STOP is present: the loop and ratification are paused until a human removes it")
    w("")
    w("ACTIVE CLAIMS (advisory, never locks; claim_hours %s)" % setting(con, "claim_hours"))
    n = 0
    for cid, c in t.claims.items():
        if t.claim_status(cid) == "active":
            w("  %-38s %-16s until %-20s %s" % (c["key"], trunc(c["agent"], 16), c["expires"], trunc(c["summary"], 60)))
            n += 1
    if not n:
        w("  none")
    w("")
    w("OPEN THREADS")
    n = 0
    for key in t.order:
        if t.thread_status(key) == "closed":
            continue
        w("  %-38s %2d posts  last %-20s %s" % (key, t.nposts[key], t.last[key], trunc(t.lastsum[key], 60)))
        n += 1
    if not n:
        w("  none")
    w("")
    w("PROPOSALS FOR THE HUMAN (the agents agreed; only the human decides - `pulse decide POST_ID yes|no`)")
    n = 0
    for key, pr in t.prop.items():
        if t.proposal_status(key) == "awaiting":
            w("  awaiting   %-38s since %-20s %s" % (key, pr["date"], trunc(pr["summary"], 60)))
            w("             read: pulse show %s" % pr["id"])
            n += 1
    for key, pr in t.prop.items():
        st = t.proposal_status(key)
        if st != "awaiting":
            w("  %-10s %-38s on %-23s %s" % (st, key, t.res_date[key], trunc(pr["summary"], 60)))
            n += 1
    if not n:
        w("  none")
    w("")
    w("EXPERIMENT QUEUE (score = up - down; quorum %d distinct voters; unvoted expire after %d days)" % (params["quorum"], params["expiry_days"]))
    n = 0
    for s in t.suggestion_records():
        if s["status"] == "closed":
            continue
        w("  %+3d  %-12s %-38s %-44s up %d down %d voters %d  %s" % (s["score"], s["status"], s["key"], trunc(s["id"], 44), s["up"], s["down"], s["voters"], trunc(s["summary"], 50)))
        n += 1
    if not n:
        w("  none")
    w("")
    amends, _ = t.amendment_records()
    w("AMENDMENTS (supermajority %d/%d of counted votes; quorum %d distinct sessions, proposer excluded)" % (params["amend_num"], params["amend_den"], params["amend_quorum"]))
    n = 0
    for a in amends:
        if not a["open"]:
            continue
        w("  %-38s %s" % (a["key"], trunc(a["id"], 60)))
        w("      %s" % a["status"])
        w("      up %d down %d  voters %d/%d  %d%% up  identities %d  uncounted: %d self, %d hash mismatch, %d no Read line"
          % (a["up"], a["down"], a["voters"], a["quorum"], a["pct"], a["nid"], a["self"], a["badhash"], a["noread"]))
        n += 1
    if not n:
        w("  none open")
    w("")
    w("STALE (expired claims, threads idle > %d days)" % params["stale_days"])
    n = 0
    for cid, c in t.claims.items():
        st = t.claim_status(cid)
        if st in ("expired", "no expiry"):
            w("  claim      %-38s %-16s expired %s" % (c["key"], trunc(c["agent"], 16), c["expires"] or "(no expiry)"))
            n += 1
    for key in t.order:
        if t.thread_status(key) == "stale":
            w("  thread     %-38s idle since %s" % (key, t.last[key]))
            n += 1
    if t.nokind:
        w("  %d post(s) without a kind" % t.nokind)
        n += 1
    if not n:
        w("  none")
    w("")
    if slug != COMMONS_SLUG:
        w("JOBS (the runner's queue for this project; pulse jobs for every project)")
        jobs = con.execute("SELECT * FROM jobs WHERE project = ? AND status IN ('queued', 'leased', 'blocked') ORDER BY status, priority DESC, created_at", (slug,)).fetchall()
        for j in jobs:
            extra = " by %s until %s" % (j["lease_agent"], j["lease_until"]) if j["status"] == "leased" else ""
            w("  %-7s %-8s %-28s tier %d  %s%s" % (j["status"], j["kind"], trunc(j["id"], 28), j["tier"], trunc(j["title"], 50), extra))
        if not jobs:
            w("  none queued")
        w("")
    w("AGENTS (unique names; pruned after %s days without useful output)" % setting(con, "prune_after_days"))
    rows = agent_scorecards(con, include_pruned=False)
    for r in rows:
        w("  %-16s %-7s %-8s posts %-4d jobs %d/%d  last useful %s" % (r["name"], r["status"], r["kind"], r["posts"], r["jobs_done"], r["jobs_done"] + r["jobs_failed"], r["last_useful_at"] or "-"))
    if not rows:
        w("  none joined yet: pulse agent join --name NAME --runner R --model M")
    return "\n".join(out) + "\n"


def show_post(con, post_id):
    row = post_row(con, post_id)
    if row is None:
        raise PulseError("No post %r" % post_id, 2)
    out = ["---"]
    for k, v in row_frontmatter(row).items():
        if k in ("path",):
            continue
        out.append("%s: %s" % (k, v))
    out.append("project: %s" % row["project"])
    out.append("---")
    out.append(row["body"].rstrip("\n"))
    return "\n".join(out) + "\n"


# --------------------------------------------------------------------------- #
# Decisions: the human's yes or no, recorded once, as data
# --------------------------------------------------------------------------- #

def decide(con, post_id, answer, words="", decided_by=HUMAN_AGENT):
    if answer not in ("yes", "no"):
        raise PulseError("The answer is yes or no", 2)
    target = post_row(con, post_id)
    if target is None:
        raise PulseError("No post %r" % post_id, 2)
    if con.execute("SELECT 1 FROM decisions WHERE post_id = ?", (post_id,)).fetchone():
        raise PulseError("Post %s was already decided; a new proposal needs a new post" % post_id, 2)
    resolution = "accepted" if answer == "yes" else "declined"
    body = "The human answered **%s**.\n\n%s\n\nProposal: %s\n" % (answer, words or "(no words recorded)", post_id)
    human = agent_row(con, HUMAN_AGENT)
    post = create_post(con, "result", target["project"], target["area"], target["thread"],
                       "%s: %s" % (resolution, trunc(target["summary"], 200)), body, agent=human, on=post_id,
                       resolution=resolution, session="", branch="")
    con.execute("INSERT INTO decisions (post_id, project, answer, words, decided_at, decided_by, result_post) VALUES (?, ?, ?, ?, ?, ?, ?)",
                (post_id, target["project"], answer, words or "", utc_now(), decided_by, post["id"]))
    event(con, decided_by, "decide", post_id, {"answer": answer, "result": post["id"]})
    return post


# --------------------------------------------------------------------------- #
# Jobs and leases
# --------------------------------------------------------------------------- #

def new_id(prefix):
    return "%s-%s-%s" % (prefix, stamp_of(utc_now()), hashlib.sha1(os.urandom(8)).hexdigest()[:6])


def add_job(con, project, kind, title, brief="", tier=1, priority=0, source="", created_by="", dedupe_source=True):
    if kind not in JOB_KINDS:
        raise PulseError("Job kind must be one of: %s" % ", ".join(JOB_KINDS), 2)
    if project_by_slug(con, project) is None:
        raise PulseError("No project %r" % project, 2)
    if int(tier) not in (0, 1):
        raise PulseError("The runner takes Tier 0 and Tier 1 jobs only; Tier 2 needs an ADR and the human", 2)
    if dedupe_source and source:
        dup = con.execute("SELECT id FROM jobs WHERE source = ? AND status IN ('queued', 'leased', 'blocked')", (source,)).fetchone()
        if dup:
            return dict(con.execute("SELECT * FROM jobs WHERE id = ?", (dup["id"],)).fetchone()), False
    jid = new_id("job")
    con.execute("INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?)",
                (jid, project, kind, int(tier), title, brief, source, int(priority), utc_now(), created_by))
    event(con, created_by, "job.add", jid, {"project": project, "kind": kind, "title": title})
    return dict(con.execute("SELECT * FROM jobs WHERE id = ?", (jid,)).fetchone()), True


def lease_job(con, agent, project=None, job_id=None, minutes=None, now=None):
    """Atomically take the next queued job. One leased job per project at a time."""
    now = now or utc_now()
    minutes = minutes if minutes is not None else setting_int(con, "minutes_per_job") + 5
    con.execute("BEGIN IMMEDIATE")
    try:
        expired = con.execute("SELECT id FROM jobs WHERE status = 'leased' AND lease_until != '' AND lease_until < ?", (now,)).fetchall()
        for row in expired:
            con.execute("UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '' WHERE id = ?", (row["id"],))
            event(con, "lease", "job.requeue", row["id"], "lease expired")
        sql = ("SELECT j.* FROM jobs j JOIN projects p ON p.slug = j.project WHERE j.status = 'queued' AND p.status = 'active' "
               "AND NOT EXISTS (SELECT 1 FROM jobs k WHERE k.project = j.project AND k.status = 'leased')")
        args = []
        if project:
            sql += " AND j.project = ?"
            args.append(project)
        if job_id:
            sql += " AND j.id = ?"
            args.append(job_id)
        sql += " ORDER BY j.priority DESC, j.created_at LIMIT 1"
        row = con.execute(sql, args).fetchone()
        if row is None:
            con.execute("COMMIT")
            return None
        until = plus_hours(now, minutes / 60.0)
        con.execute("UPDATE jobs SET status = 'leased', lease_agent = ?, lease_until = ?, generation = generation + 1, attempts = attempts + 1 "
                    "WHERE id = ? AND status = 'queued'", (agent, until, row["id"]))
        event(con, agent, "job.lease", row["id"], {"until": until})
        con.execute("COMMIT")
    except Exception:
        con.execute("ROLLBACK")
        raise
    return dict(con.execute("SELECT * FROM jobs WHERE id = ?", (row["id"],)).fetchone())


def finish_job(con, job_id, agent, status, result="", branch=""):
    if status not in ("done", "failed", "cancelled", "blocked", "queued"):
        raise PulseError("A job ends done, failed, cancelled, blocked, or back to queued", 2)
    job = con.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone()
    if job is None:
        raise PulseError("No job %r" % job_id, 2)
    if job["status"] == "leased" and job["lease_agent"] and agent and job["lease_agent"] != agent and agent != HUMAN_AGENT:
        raise PulseError("Job %s is leased by %s, not %s" % (job_id, job["lease_agent"], agent), 2)
    con.execute("UPDATE jobs SET status = ?, result = ?, branch = COALESCE(NULLIF(?, ''), branch), finished_at = ?, "
                "lease_agent = CASE WHEN ? = 'queued' THEN '' ELSE lease_agent END, lease_until = '' WHERE id = ?",
                (status, result, branch, utc_now() if status != "queued" else "", status, job_id))
    event(con, agent, "job.%s" % status, job_id, result[:400])
    return dict(con.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone())


# --------------------------------------------------------------------------- #
# The runner: bounded, sandboxed-by-clone, never pushes, never merges
# --------------------------------------------------------------------------- #

DENIED_TOOLS = ("shell(git push)", "shell(git remote)", "shell(gh:*)")
RESULT_FILE = "PULSE_RESULT.md"


def runner_stopped(con):
    if setting(con, "stop") == "1":
        return "the runner is stopped in the store (pulse start to resume)"
    brain = brain_project(con)
    if brain and (Path(brain["path"]) / "docs" / "evolution" / "STOP").exists():
        return "docs/evolution/STOP is present in the brain (%s); a human removes it" % brain["path"]
    return ""


def runs_today(con, now=None):
    day = (now or utc_now())[:10]
    return con.execute("SELECT COUNT(*) FROM runs WHERE mode != 'dry-run' AND substr(started_at, 1, 10) = ?", (day,)).fetchone()[0]


def write_brief(job, project, agent, clone, minutes):
    brief = []
    w = brief.append
    w("# PULSE job %s for %s (agent %s)" % (job["id"], project["slug"], agent))
    w("")
    w("You are the agent `%s`, working alone in a throwaway clone at %s." % (agent, clone))
    w("The real checkout is elsewhere and you cannot reach it. Nothing you do here is merged; the runner")
    w("copies your commits to a local branch `pulse/%s/%s` for a human to review." % (agent, job["id"]))
    w("")
    w("This is your only run: nobody reads your chat output and there is no second turn. Do not stop after a")
    w("preamble or a plan - do the task now with your tools, then write %s. A run that leaves no %s" % (RESULT_FILE, RESULT_FILE))
    w("counts as failed.")
    w("")
    w("## Task (%s, Tier %d)" % (job["kind"], job["tier"]))
    w("")
    w(job["title"])
    w("")
    if job["brief"]:
        w(job["brief"])
        w("")
    w("## Rules")
    w("")
    w("1. Read AGENTS.md first when it exists; the project's rules apply in full.")
    w("2. Tier 0 and Tier 1 changes only. If the task needs a real decision (money, auth, identity, data,")
    w("   privacy, migrations, dependencies, deploy, publish, security posture, or a surface no ADR covers)")
    w("   or crosses a hard stop, stop and explain that in %s instead of doing it." % RESULT_FILE)
    w("3. Never push, never add remotes, never call gh, never touch credentials, never install packages.")
    w("4. Verify with the project's own checks%s before committing." % (" (`%s`)" % project["verify_cmd"] if project["verify_cmd"] else ""))
    w("5. Commit small, signed-off-by nobody: `git -c user.name=%s -c user.email=%s@pulse.local commit`." % (agent, agent))
    w("6. Finish within %d minutes. Write %s at the clone root (do not commit it) with:" % (minutes, RESULT_FILE))
    w("   - `SUMMARY:` one line saying what changed and what you measured")
    w("   - `QUESTION:` one line when you stopped for a Tier 2 decision or a hard stop (optional)")
    w("   - `FINDING:` one line for anything the next agent would rediscover (optional, repeatable)")
    return "\n".join(brief) + "\n"


def parse_result_file(clone):
    path = Path(clone) / RESULT_FILE
    out = {"summary": "", "questions": [], "findings": [], "found": False}
    if not path.is_file():
        return out
    out["found"] = True
    for line in path.read_text(encoding="utf-8", errors="replace").split("\n"):
        m = re.match(r"^\s*-?\s*`?(SUMMARY|QUESTION|FINDING)`?:\s*(.+)$", line)
        if not m:
            continue
        key, text = m.group(1), " ".join(m.group(2).split())
        if key == "SUMMARY" and not out["summary"]:
            out["summary"] = text
        elif key == "QUESTION":
            out["questions"].append(text)
        elif key == "FINDING":
            out["findings"].append(text)
    return out


def worker_command(con, brief_text, clone, worker_cmd=None):
    if worker_cmd:
        return shlex.split(worker_cmd), True
    cmd = [setting(con, "runner_cmd") or "copilot", "-p", brief_text, "-C", str(clone), "--allow-all-tools"]
    for pattern in DENIED_TOOLS:
        cmd += ["--deny-tool", pattern]
    cmd += ["--disable-builtin-mcps", "--no-ask-user", "-s", "--max-ai-credits", setting(con, "max_ai_credits")]
    model = setting(con, "worker_model")
    if model:
        cmd += ["--model", model]
    return cmd, False


def run_job(con, job, agent, dry_run=False, worker_cmd=None, home=None, now=None):
    """Run one leased job in a clone; land commits on a local branch of the real checkout; record everything."""
    now = now or utc_now()
    project = project_by_slug(con, job["project"])
    if project is None:
        raise PulseError("Job %s names an unknown project %s" % (job["id"], job["project"]), 2)
    root = Path(project["path"])
    if not (root / ".git").exists():
        raise PulseError("Project %s at %s is not a git checkout" % (project["slug"], root), 2)
    minutes = setting_int(con, "minutes_per_job")
    home_dir = pulse_home(home)
    work_dir = home_dir / "work"
    logs_dir = home_dir / "logs"
    work_dir.mkdir(parents=True, exist_ok=True)
    logs_dir.mkdir(parents=True, exist_ok=True)
    clone = work_dir / job["id"]
    if clone.exists():
        shutil.rmtree(clone)
    run_id = new_id("run")
    mode = "dry-run" if dry_run else ("custom" if worker_cmd else "real")
    log_path = logs_dir / ("%s.log" % run_id)
    con.execute("INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode) VALUES (?, ?, ?, ?, ?, 'running', ?, ?, ?)",
                (run_id, job["id"], agent, project["slug"], now, str(clone), str(log_path), mode))
    event(con, agent, "run.start", run_id, {"job": job["id"], "mode": mode})
    started = time.time()
    log = []
    status, exit_code, commits, branch, verify_note, summary = "failed", None, 0, "", "", ""
    parsed = {"summary": "", "questions": [], "findings": []}
    try:
        clone_args = ["clone", "-q", "--no-tags"]
        if project["default_branch"] and project["default_branch"] != "HEAD":
            clone_args += ["-b", project["default_branch"]]
        out = subprocess.run(["git"] + clone_args + [str(root), str(clone)], capture_output=True, text=True, timeout=300)
        log.append("$ git %s\n%s%s" % (" ".join(clone_args), out.stdout, out.stderr))
        if out.returncode != 0:
            raise PulseError("clone failed: %s" % out.stderr.strip())
        git(clone, "remote", "remove", "origin")
        base = git(clone, "rev-parse", "HEAD", check=True).stdout.strip()
        brief_text = write_brief(job, project, agent, clone, minutes)
        brief_path = work_dir / ("%s.brief.md" % job["id"])
        brief_path.write_text(brief_text, encoding="utf-8")
        if dry_run:
            status, exit_code = "dry-run", 0
            summary = "dry run: clone prepared at %s, brief written, no worker started" % clone
            log.append(summary)
        else:
            cmd, custom = worker_command(con, brief_text, clone, worker_cmd)
            env = dict(os.environ)
            env.update({"PULSE_JOB": job["id"], "PULSE_BRIEF": str(brief_path), "PULSE_CLONE": str(clone),
                        "PULSE_AGENT": agent, "PULSE_PROJECT": project["slug"], "GIT_TERMINAL_PROMPT": "0"})
            env.pop("PULSE_HOME", None)
            log.append("$ %s" % " ".join(shlex.quote(c) if c != brief_text else "<brief>" for c in cmd))
            try:
                proc = subprocess.run(cmd, cwd=str(clone), env=env, capture_output=True, text=True, timeout=minutes * 60)
                exit_code = proc.returncode
                log.append(proc.stdout[-20000:])
                log.append(proc.stderr[-20000:])
            except subprocess.TimeoutExpired as exc:
                status, exit_code = "timeout", None
                log.append("timeout after %d minutes" % minutes)
                log.append((exc.stdout or b"")[-20000:].decode("utf-8", "replace") if isinstance(exc.stdout, bytes) else (exc.stdout or "")[-20000:])
            except FileNotFoundError:
                raise PulseError("worker command not found: %s (setting runner_cmd=%s)" % (cmd[0], setting(con, "runner_cmd")))
            parsed = parse_result_file(clone)
            if status != "timeout":
                rev = git(clone, "rev-list", "--count", "%s..HEAD" % base)
                commits = int(rev.stdout.strip() or 0) if rev.returncode == 0 else 0
                dirty = git(clone, "status", "--porcelain", "--untracked-files=no").stdout.strip()
                if dirty:
                    log.append("uncommitted changes left in the clone were discarded:\n%s" % dirty)
                if exit_code != 0:
                    status = "failed"
                    summary = "worker exited %s" % exit_code
                elif not parsed["found"]:
                    # The worker answered without acting (a preamble, a plan) or never reported.
                    status = "failed"
                    summary = "worker wrote no %s; nothing to land" % RESULT_FILE
                elif commits == 0:
                    status = "done"
                    summary = parsed["summary"] or "no commits; nothing to land"
                else:
                    ok = True
                    if project["verify_cmd"]:
                        try:
                            ver = subprocess.run(project["verify_cmd"], shell=True, cwd=str(clone), capture_output=True, text=True,
                                                 timeout=max(60, minutes * 60))
                            verify_note = "verify exit %d" % ver.returncode
                            log.append("$ %s\n%s%s" % (project["verify_cmd"], ver.stdout[-20000:], ver.stderr[-20000:]))
                            ok = ver.returncode == 0
                        except subprocess.TimeoutExpired:
                            verify_note, ok = "verify timed out", False
                    if not ok:
                        status = "failed"
                        summary = "%d commit(s) but %s; nothing landed" % (commits, verify_note)
                    else:
                        branch = "pulse/%s/%s" % (agent, job["id"])
                        exists = git(root, "rev-parse", "--verify", "--quiet", "refs/heads/%s" % branch)
                        if exists.returncode == 0:
                            status = "failed"
                            summary = "branch %s already exists in %s; nothing landed" % (branch, root)
                            branch = ""
                        else:
                            fetched = git(root, "fetch", "-q", "--no-tags", str(clone), "HEAD:refs/heads/%s" % branch, timeout=300)
                            log.append("$ git fetch <clone> HEAD:refs/heads/%s\n%s" % (branch, fetched.stderr))
                            if fetched.returncode != 0:
                                status, branch = "failed", ""
                                summary = "could not land the branch: %s" % fetched.stderr.strip()
                            else:
                                status = "done"
                                summary = parsed["summary"] or "%d commit(s) landed on %s" % (commits, branch)
                                if verify_note:
                                    summary += " (%s)" % verify_note
    except PulseError as exc:
        status, summary = "failed", str(exc)
        log.append(summary)
    except Exception as exc:  # the run must always be recorded
        status, summary = "failed", "runner error: %r" % (exc,)
        log.append(summary)
    finally:
        seconds = round(time.time() - started, 1)
        try:
            log_path.write_text("\n".join(l for l in log if l), encoding="utf-8")
        except OSError:
            pass
        if clone.exists():
            shutil.rmtree(clone, ignore_errors=True)
    con.execute("UPDATE runs SET finished_at = ?, status = ?, exit_code = ?, seconds = ?, branch = ?, commits = ?, summary = ?, verify = ? WHERE id = ?",
                (utc_now(), status, exit_code, seconds, branch, commits, summary, verify_note, run_id))
    job_status = "done" if status in ("done", "dry-run") else "failed"
    if status == "dry-run":
        job_status = "queued"
    finish_job(con, job["id"], agent, job_status, summary, branch)
    if job_status == "done":
        con.execute("UPDATE agents SET jobs_done = jobs_done + 1 WHERE name = ?", (agent,))
        touch_agent(con, agent, useful=commits > 0)
    elif job_status == "failed":
        con.execute("UPDATE agents SET jobs_failed = jobs_failed + 1 WHERE name = ?", (agent,))
    if status != "dry-run":
        body = ["Run `%s` of job `%s` (%s, Tier %d) by %s: **%s** in %ss." % (run_id, job["id"], job["kind"], job["tier"], agent, status, seconds), ""]
        body.append("- Commits: %d%s" % (commits, "; branch `%s` (local, human-merged)" % branch if branch else ""))
        if verify_note:
            body.append("- Verify: %s" % verify_note)
        body.append("- Log: %s" % log_path)
        for f in parsed["findings"]:
            body.append("- Finding: %s" % f)
        arow = agent_row(con, agent)
        create_post(con, "result", project["slug"], "jobs", job["id"], trunc("%s: %s" % (status, summary), 230), "\n".join(body) + "\n",
                    agent=arow or agent, check_agent=bool(arow), branch=branch, resolution="tested" if job_status == "done" else "")
        for q in parsed["questions"]:
            create_post(con, "question", project["slug"], "jobs", job["id"], trunc(q, 230),
                        "The worker stopped because this needs a real decision or crosses a hard stop.\n\n%s\n" % q,
                        agent=arow or agent, check_agent=bool(arow), branch=branch)
    event(con, agent, "run.finish", run_id, {"status": status, "commits": commits, "branch": branch})
    return dict(con.execute("SELECT * FROM runs WHERE id = ?", (run_id,)).fetchone())


def work_once(con, agent, project=None, job_id=None, dry_run=False, worker_cmd=None, home=None, now=None):
    """Lease one job and run it. Returns the run row, or None with a reason string."""
    now = now or utc_now()
    why = runner_stopped(con)
    if why:
        return None, why
    if not dry_run:
        cap = setting_int(con, "jobs_per_day")
        done = runs_today(con, now)
        if done >= cap:
            return None, "daily cap reached: %d of %d jobs today (setting jobs_per_day)" % (done, cap)
    job = lease_job(con, agent, project=project, job_id=job_id, now=now)
    if job is None:
        return None, "no queued job" + (" for %s" % project if project else "")
    run = run_job(con, job, agent, dry_run=dry_run, worker_cmd=worker_cmd, home=home, now=now)
    return run, ""


def open_questions(con, slug, tally):
    out = []
    answered = set()
    for r in tally.rows:
        if r["kind"] == "answer" and r["on_post"]:
            answered.add(r["on_post"])
    for r in tally.rows:
        if r["kind"] != "question" or r["area"] == "jobs":
            continue
        key = "%s/%s" % (r["area"], r["thread"])
        if tally.thread_status(key) == "closed" or r["id"] in answered:
            continue
        if any(x["kind"] == "answer" and x["area"] == r["area"] and x["thread"] == r["thread"] and x["date"] > r["date"] for x in tally.rows):
            continue
        out.append(r)
    return out


def plan_jobs(con, slug, now=None, created_by="daemon"):
    """Create the next job for a project that has none waiting: improve, answer, or reflect."""
    now = now or utc_now()
    waiting = con.execute("SELECT COUNT(*) FROM jobs WHERE project = ? AND status IN ('queued', 'leased')", (slug,)).fetchone()[0]
    if waiting:
        return []
    t, params = tally_for(con, slug, None, now)
    created = []
    for s in t.suggestion_records():
        if s["status"] != "eligible":
            continue
        post = post_row(con, s["id"])
        brief = ("The agents voted this the top eligible suggestion (score %+d, %d voters). Thread %s.\n\n%s\n\n"
                 "One variable, measured by the project's own checks. When the change is kept, say what moved in SUMMARY:."
                 % (s["score"], s["voters"], s["key"], (post or {}).get("body", "")))
        job, new = add_job(con, slug, "improve", "Implement suggestion %s: %s" % (s["id"], s["summary"]), brief, tier=1,
                           priority=10 + s["score"], source="suggestion:%s" % s["id"], created_by=created_by)
        if new:
            created.append(job)
        break
    if not created:
        # One job per question, ever: a cancelled or failed attempt means an agent
        # could not answer it (often it is addressed to the human), so move on.
        tried = {r["source"] for r in con.execute("SELECT source FROM jobs WHERE source LIKE 'question:%'")}
        for q in [q for q in open_questions(con, slug, t) if "question:%s" % q["id"] not in tried][:1]:
            brief = ("Question %s in thread %s/%s:\n\n%s\n\nInvestigate in the clone and write the answer as SUMMARY:; the runner posts it. "
                     "If only a human can answer it, write QUESTION: with what you need and stop.") % (
                q["id"], q["area"], q["thread"], q["body"])
            job, new = add_job(con, slug, "answer", "Answer question %s: %s" % (q["id"], q["summary"]), brief, tier=0, priority=5,
                               source="question:%s" % q["id"], created_by=created_by)
            if new:
                created.append(job)
    if not created:
        source = "reflect:%s:%s" % (slug, now[:10])
        done_today = con.execute("SELECT 1 FROM jobs WHERE source = ?", (source,)).fetchone()
        if not done_today:
            brief = ("Run the project's own checks (tests, lint, links, `git diff --check`), read docs/memory/ and the open ADRs' "
                     "acceptance criteria, and fix one Tier 0 thing you can prove with the checks (a flaky test, a broken link, a "
                     "stale doc, a lint finding). Record every other signal as FINDING: lines so the runner posts them.")
            job, new = add_job(con, slug, "reflect", "Daily reflect: find and fix one provable Tier 0 issue", brief, tier=0, priority=1,
                               source=source, created_by=created_by)
            if new:
                created.append(job)
    return created


def pick_worker(con, agent=None):
    if agent:
        return resolve_agent(con, agent)["name"]
    row = con.execute("SELECT a.name FROM agents a WHERE a.status = 'active' AND a.kind = 'worker' AND NOT EXISTS "
                      "(SELECT 1 FROM jobs j WHERE j.status = 'leased' AND j.lease_agent = a.name) ORDER BY a.last_active_at DESC LIMIT 1").fetchone()
    if row:
        return row["name"]
    return spawn_agent(con, runner=setting(con, "runner_cmd"), model=setting(con, "worker_model"), kind="worker")["name"]


def daemon(con, agent=None, interval=300, dry_run=False, worker_cmd=None, home=None, max_loops=None, out=sys.stdout):
    loops = 0
    last_prune_day = ""
    print("daemon started %s (interval %ss%s)" % (utc_now(), interval, ", dry run" if dry_run else ""), file=out, flush=True)
    while True:
        loops += 1
        now = utc_now()
        why = runner_stopped(con)
        if why:
            print("stopped: %s" % why, file=out, flush=True)
            return "stopped"
        if now[:10] != last_prune_day:
            for name, reason in prune_agents(con, now):
                print("pruned %s: %s" % (name, reason), file=out, flush=True)
            last_prune_day = now[:10]
        for proj in project_rows(con):
            for job in plan_jobs(con, proj["slug"], now):
                print("planned %s %s: %s" % (job["kind"], job["id"], job["title"]), file=out, flush=True)
        worker = pick_worker(con, agent)
        run, reason = work_once(con, worker, dry_run=dry_run, worker_cmd=worker_cmd, home=home, now=now)
        if run:
            print("%s %s %s: %s" % (run["status"], run["job_id"], run["agent"], run["summary"]), file=out, flush=True)
        else:
            print("idle: %s" % reason, file=out, flush=True)
        if max_loops is not None and loops >= max_loops:
            return "done"
        if run is None or reason.startswith("daily cap"):
            time.sleep(interval)


# --------------------------------------------------------------------------- #
# Migration from the Markdown boards and the JSON registry (idempotent)
# --------------------------------------------------------------------------- #

def load_json_registry(path):
    try:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def import_registry(con, registry_path=None, actor="migrate"):
    path = Path(registry_path) if registry_path else pulse_home() / "projects.json"
    data = load_json_registry(path)
    imported = []
    for p in data.get("projects", []):
        if not isinstance(p, dict) or not p.get("path") or not Path(p["path"]).is_dir():
            continue
        proj, created = add_project(con, p["path"], remote=p.get("remote") or "", brain=bool(p.get("brain")), actor=actor,
                                    slug=p.get("slug"))
        imported.append((proj["slug"], created))
    commons = data.get("commons") or ""
    if commons:
        con.execute("INSERT INTO meta (key, value) VALUES ('commons_path', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", (commons,))
    return imported


def legacy_agent_name(con, legacy_id, call_sign):
    base = slugify(call_sign) if call_sign else slugify(legacy_id.split("/", 1)[-1] if "/" in legacy_id else legacy_id)
    if not valid_name(base):
        base = "agent-" + re.sub(r"[^a-z0-9-]", "", base)[:30]
        if not valid_name(base):
            base = "agent-" + hashlib.sha1(legacy_id.encode()).hexdigest()[:8]
    return unique_slug(con, base, "agents", "name")


def import_board(con, repo, project_slug, actor="migrate"):
    """Import docs/board/<area>/<thread>/*.md of one checkout as immutable rows. Skips posts already imported."""
    root = Path(repo).resolve()
    board = root / "docs" / "board"
    counts = {"posts": 0, "skipped": 0, "agents": 0, "quarantined": 0}
    if not board.is_dir():
        return counts
    files = sorted(str(p) for p in board.glob("*/*/*.md"))
    parsed = []
    for f in files:
        text = Path(f).read_text(encoding="utf-8", errors="replace")
        fm, body = split_frontmatter(text)
        rel = str(Path(f).relative_to(root))
        parts = Path(f).parts
        parsed.append((rel, parts[-3], parts[-2], Path(f).stem, fm, body))
    # one agent per distinct legacy id, named by the call sign it used most;
    # when two ids shared a call sign the heavier user keeps it, the other
    # falls back to its next call sign or its model
    legacy = {}
    for rel, area, thread, pid, fm, body in parsed:
        lid = fm.get("agent", "")
        if not lid:
            continue
        cur = legacy.setdefault(lid, {"signs": {}, "runner": fm.get("runner", ""), "model": fm.get("model", ""), "sessions": [], "posts": 0})
        cur["posts"] += 1
        if fm.get("name"):
            sign = cur["signs"].setdefault(fm["name"], {"count": 0, "latest": ""})
            sign["count"] += 1
            sign["latest"] = max(sign["latest"], fm.get("date", ""))
        if fm.get("session") and fm["session"] not in cur["sessions"]:
            cur["sessions"].append(fm["session"])
    names = {}
    for lid in list(legacy):
        row = con.execute("SELECT name FROM agents WHERE legacy_id = ?", (lid,)).fetchone()
        if row:
            names[lid] = row["name"]
    candidates = sorted(((info["count"], info["latest"], lid, sign) for lid, cur in legacy.items() for sign, info in cur["signs"].items()),
                        reverse=True)
    chosen, taken = {}, {r["name"] for r in con.execute("SELECT name FROM agents")}
    for count, latest, lid, sign in candidates:
        slug = slugify(sign)
        if lid in names or lid in chosen or slug in taken or not valid_name(slug):
            continue
        chosen[lid], taken = (slug, sign), taken | {slug}
    for lid, info in sorted(legacy.items(), key=lambda kv: -kv[1]["posts"]):
        if lid in names:
            continue
        slug, display = chosen.get(lid, (legacy_agent_name(con, lid, ""), ""))
        if slug in {r["name"] for r in con.execute("SELECT name FROM agents")}:
            slug = unique_slug(con, slug, "agents", "name")
        join_agent(con, slug, runner=info["runner"], model=info["model"], display=display or slug, kind="legacy", legacy_id=lid)
        if info["sessions"]:
            con.execute("UPDATE agents SET sessions = ? WHERE name = ?", (json.dumps(info["sessions"][-50:]), slug))
        names[lid] = slug
        counts["agents"] += 1
    stem_last = {}
    for rel, area, thread, pid, fm, body in parsed:
        stem_last[pid] = rel
    for rel, area, thread, pid, fm, body in parsed:
        origin = "markdown:%s" % rel
        if con.execute("SELECT 1 FROM posts WHERE origin = ? AND project = ?", (origin, project_slug)).fetchone():
            counts["skipped"] += 1
            continue
        lid = fm.get("agent", "")
        if not lid or not fm.get("date"):
            counts["quarantined"] += 1
            event(con, actor, "migrate.quarantine", rel, "missing agent or date")
            continue
        # Markdown ids were unique per path, not per board. board.sh keyed a
        # reused stem by its last file in path order, so that file keeps the
        # bare id (votes on it still count) and earlier ones get their thread;
        # a stem already used by another project gets the project too.
        post_id = pid
        if stem_last.get(pid) != rel:
            post_id = "%s-%s" % (pid, thread)
        if con.execute("SELECT 1 FROM posts WHERE id = ?", (post_id,)).fetchone():
            post_id = unique_post_id(con, "%s-%s" % (post_id, project_slug))
        extra = {k: v for k, v in fm.items() if k not in ("kind", "agent", "runner", "model", "session", "date", "branch", "expires",
                                                          "on", "vote", "resolution", "summary", "text_hash", "links", "files",
                                                          "name", "project")}
        con.execute(
            "INSERT INTO posts (id, project, area, thread, kind, agent, runner, model, session, date, branch, expires, on_post, vote, "
            "text_hash, has_read_line, resolution, summary, body, links, files, name, origin_project, extra, origin, created_at) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (post_id, project_slug, area, thread, fm.get("kind", ""), names[lid], fm.get("runner", ""), fm.get("model", ""),
             fm.get("session", ""), fm.get("date", ""), fm.get("branch", ""), fm.get("expires", ""), fm.get("on", ""), fm.get("vote", ""),
             fm.get("text_hash", ""), has_read_line(body), fm.get("resolution", ""), fm.get("summary", ""), body, fm.get("links", ""),
             fm.get("files", ""), fm.get("name", ""), fm.get("project", ""), json.dumps(extra, sort_keys=True), origin, utc_now()))
        counts["posts"] += 1
    # counters: posts, last active, last useful
    for lid, name in names.items():
        stats = con.execute("SELECT COUNT(*) AS n, MAX(date) AS last, MAX(CASE WHEN kind IN (%s) THEN date ELSE '' END) AS useful "
                            "FROM posts WHERE agent = ?" % ",".join("?" * len(USEFUL_KINDS)), tuple(USEFUL_KINDS) + (name,)).fetchone()
        con.execute("UPDATE agents SET posts = ?, last_active_at = MAX(last_active_at, ?), last_useful_at = MAX(last_useful_at, ?) WHERE name = ?",
                    (stats["n"], stats["last"] or "", stats["useful"] or "", name))
    event(con, actor, "migrate.board", project_slug, counts)
    return counts


def migrate(con, repo=None, commons=None, registry=None, actor="migrate"):
    report = {"registry": import_registry(con, registry, actor)}
    if repo:
        proj = project_for_path(con, repo)
        if proj is None:
            proj, _ = add_project(con, repo, actor=actor)
        report["board"] = (proj["slug"], import_board(con, repo, proj["slug"], actor))
    commons_path = commons or (con.execute("SELECT value FROM meta WHERE key = 'commons_path'").fetchone() or {"value": ""})["value"] \
        or str(pulse_home() / "commons")
    if Path(commons_path, "docs", "board").is_dir():
        con.execute("INSERT INTO meta (key, value) VALUES ('commons_path', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", (commons_path,))
        report["commons"] = (commons_path, import_board(con, commons_path, COMMONS_SLUG, actor))
    return report


# --------------------------------------------------------------------------- #
# Export, schema, and what the dashboard reads
# --------------------------------------------------------------------------- #

def export_store(con, sql_path=None, db_path=None, force=False):
    target = Path(sql_path or db_path).expanduser().resolve()
    for p in project_rows(con, include_inactive=True):
        if is_inside(target, p["path"]):
            raise PulseError("Refusing to export into the registered project %s (%s); pick a path outside every checkout" % (p["slug"], p["path"]), 2)
    if target.exists() and not force:
        raise PulseError("%s exists; pass --force to overwrite" % target, 2)
    target.parent.mkdir(parents=True, exist_ok=True)
    if sql_path:
        with open(target, "w", encoding="utf-8") as fh:
            for line in con.iterdump():
                fh.write(line + "\n")
    else:
        dst = sqlite3.connect(str(target))
        con.backup(dst)
        dst.close()
    return target


def schema_text():
    return SCHEMA_SQL.strip() + "\n"


def open_store(home=None, readonly=True):
    return connect(home, create=False, readonly=readonly)


def project_for_repo(con, repo):
    return project_for_path(con, repo)


def store_fingerprint(home=None):
    path = store_path(home)
    if not path.is_file():
        return ""
    st = path.stat()
    seq = ""
    try:
        con = connect(home, readonly=True)
        try:
            row = con.execute("SELECT MAX(seq) FROM events").fetchone()
            seq = str(row[0] or 0)
        finally:
            con.close()
    except (sqlite3.Error, PulseError):
        pass
    return "store:%d:%d:%s" % (st.st_mtime_ns, st.st_size, seq)


def registry_from_store(con):
    commons = con.execute("SELECT value FROM meta WHERE key = 'commons_path'").fetchone()
    return {"version": 1, "commons": commons["value"] if commons else "",
            "projects": [{"slug": p["slug"], "path": p["path"], "remote": p["remote"], "registered": p["added_at"],
                          "brain": bool(p["brain"]), "name": p["name"], "status": p["status"]}
                         for p in project_rows(con, include_inactive=True)]}


def status_text(con):
    out = []
    w = out.append
    w("store      %s (schema %s)" % (store_path(), con.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()["value"]))
    w("projects   %d active" % len(project_rows(con)))
    for p in project_rows(con):
        w("  %-18s %s%s" % (p["slug"], p["path"], "  (brain)" if p["brain"] else ""))
    agents = con.execute("SELECT status, COUNT(*) AS n FROM agents GROUP BY status").fetchall()
    w("agents     " + ", ".join("%d %s" % (a["n"], a["status"]) for a in agents))
    w("posts      %d" % con.execute("SELECT COUNT(*) FROM posts").fetchone()[0])
    jobs = con.execute("SELECT status, COUNT(*) AS n FROM jobs GROUP BY status").fetchall()
    w("jobs       " + (", ".join("%d %s" % (j["n"], j["status"]) for j in jobs) or "none"))
    w("runs today %d of %s (jobs_per_day)" % (runs_today(con), setting(con, "jobs_per_day")))
    why = runner_stopped(con)
    w("runner     %s" % (why or "running allowed"))
    w("settings   " + ", ".join("%s=%s" % (r["name"], r["value"]) for r in con.execute("SELECT name, value FROM settings ORDER BY name")))
    return "\n".join(out) + "\n"


# --------------------------------------------------------------------------- #
# Command line
# --------------------------------------------------------------------------- #

def read_body(args):
    if getattr(args, "body_file", None):
        return Path(args.body_file).read_text(encoding="utf-8")
    if getattr(args, "body", None) is not None:
        return args.body
    if not sys.stdin.isatty():
        data = sys.stdin.read()
        return data
    return ""


def current_project(con, explicit=None):
    if explicit:
        if explicit == COMMONS_SLUG:
            return {"slug": COMMONS_SLUG, "path": "", "name": "the commons"}
        proj = project_by_slug(con, explicit)
        if proj is None:
            raise PulseError("No project %r (pulse projects lists them)" % explicit, 2)
        return proj
    proj = project_for_path(con, os.getcwd())
    if proj is None:
        raise PulseError("The current directory is not inside a registered project; pass --project SLUG or run `pulse init` here", 2)
    return proj


def build_parser():
    p = argparse.ArgumentParser(prog="pulse", description=__doc__.split("\n\n")[0])
    p.add_argument("--home", help="PULSE home (default $PULSE_HOME or ~/.pulse)")
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("init", help="register the current checkout as a project and queue its onboarding job")
    s.add_argument("--path", default=".")
    s.add_argument("--name")
    s.add_argument("--remote")
    s.add_argument("--verify", default="", help="the project's own check command, e.g. 'npm test'")
    s.add_argument("--brain", action="store_true", help="this checkout is the PULSE framework itself")
    s.add_argument("--agent")

    s = sub.add_parser("agent", help="join with a unique name, spawn a worker, or list")
    a = s.add_subparsers(dest="sub", required=True)
    j = a.add_parser("join")
    j.add_argument("--name", required=True)
    j.add_argument("--runner", default="")
    j.add_argument("--model", default="")
    j.add_argument("--session", default="")
    j.add_argument("--display", default="")
    j.add_argument("--kind", default="session", choices=("session", "worker"))
    sp = a.add_parser("spawn")
    sp.add_argument("--runner", default="")
    sp.add_argument("--model", default="")
    ls = a.add_parser("list")
    ls.add_argument("--all", action="store_true")
    pr = a.add_parser("prune")
    pr.add_argument("--days", type=int)
    pr.add_argument("--dry-run", action="store_true")
    pr.add_argument("--now")

    s = sub.add_parser("agents", help="scorecards of every agent")
    s.add_argument("--all", action="store_true")

    s = sub.add_parser("post", help="write one immutable post")
    s.add_argument("kind", choices=KINDS)
    s.add_argument("--project")
    s.add_argument("--area", required=True)
    s.add_argument("--thread", required=True)
    s.add_argument("--summary", required=True)
    s.add_argument("--body")
    s.add_argument("--body-file")
    s.add_argument("--on")
    s.add_argument("--vote", choices=("up", "down"), default="")
    s.add_argument("--expires")
    s.add_argument("--resolution", choices=RESOLUTIONS, default="")
    s.add_argument("--links", default="")
    s.add_argument("--files", default="")
    s.add_argument("--text-hash", default="")
    s.add_argument("--origin-project", default="")
    s.add_argument("--agent")
    s.add_argument("--session")
    s.add_argument("--claim-hours", type=float, help="for a claim: expiry in hours from now")

    s = sub.add_parser("claim", help="shortcut: post a claim that expires after claim_hours")
    s.add_argument("--project")
    s.add_argument("--area", required=True)
    s.add_argument("--thread", required=True)
    s.add_argument("--summary", required=True)
    s.add_argument("--hours", type=float)
    s.add_argument("--agent")

    s = sub.add_parser("vote", help="shortcut: vote up or down on a suggestion or amendment")
    s.add_argument("on")
    s.add_argument("vote", choices=("up", "down"))
    s.add_argument("--summary", required=True, help="the reason, one line")
    s.add_argument("--body", default="")
    s.add_argument("--text-hash", default="")
    s.add_argument("--agent")

    s = sub.add_parser("board", help="the digest an agent reads before work")
    s.add_argument("--project")
    s.add_argument("--area")
    s.add_argument("--now")
    s.add_argument("--agent")

    s = sub.add_parser("show", help="print one post")
    s.add_argument("post_id")

    s = sub.add_parser("queue", help="the experiment queue")
    s.add_argument("--project")
    s.add_argument("--now")

    s = sub.add_parser("ballot", help="open suggestions and amendments you can still vote on")
    s.add_argument("--project")
    s.add_argument("--agent")
    s.add_argument("--now")

    s = sub.add_parser("proposals", help="what waits for the human's yes or no, across every project")
    s.add_argument("--project")

    s = sub.add_parser("decide", help="the human records yes or no on a proposal")
    s.add_argument("post_id")
    s.add_argument("answer", choices=("yes", "no"))
    s.add_argument("--words", default="")

    s = sub.add_parser("hash", help="the text_hash of an amendment's proposed text")
    s.add_argument("post_id")

    sub.add_parser("projects", help="registered projects")

    s = sub.add_parser("jobs", help="the runner's queue")
    s.add_argument("--project")
    s.add_argument("--all", action="store_true")

    s = sub.add_parser("job", help="add or finish a job by hand")
    a = s.add_subparsers(dest="sub", required=True)
    ad = a.add_parser("add")
    ad.add_argument("--project")
    ad.add_argument("--kind", default="task", choices=JOB_KINDS)
    ad.add_argument("--title", required=True)
    ad.add_argument("--brief", default="")
    ad.add_argument("--tier", type=int, default=1)
    ad.add_argument("--priority", type=int, default=0)
    ad.add_argument("--source", default="")
    ad.add_argument("--agent")
    fi = a.add_parser("finish")
    fi.add_argument("job_id")
    fi.add_argument("status", choices=("done", "failed", "cancelled", "blocked", "queued"))
    fi.add_argument("--result", default="")
    fi.add_argument("--agent")

    s = sub.add_parser("plan", help="create the next job for projects with an empty queue")
    s.add_argument("--project")
    s.add_argument("--now")

    s = sub.add_parser("work", help="lease one job and run it in a throwaway clone")
    s.add_argument("--agent")
    s.add_argument("--project")
    s.add_argument("--job")
    s.add_argument("--dry-run", action="store_true")
    s.add_argument("--worker-cmd", help="run this command instead of the copilot worker (tests)")
    s.add_argument("--now")

    s = sub.add_parser("daemon", help="plan, lease, run, prune - until stopped")
    s.add_argument("--agent")
    s.add_argument("--interval", type=int, default=300)
    s.add_argument("--dry-run", action="store_true")
    s.add_argument("--worker-cmd")
    s.add_argument("--max-loops", type=int)

    sub.add_parser("stop", help="stop the runner (a kill switch in the store)")
    sub.add_parser("start", help="let the runner run again")

    s = sub.add_parser("settings", help="read or change a setting")
    s.add_argument("action", choices=("get", "set", "list"), nargs="?", default="list")
    s.add_argument("name", nargs="?")
    s.add_argument("value", nargs="?")
    s.add_argument("--agent")

    s = sub.add_parser("migrate", help="import a Markdown board, the JSON registry, and the commons")
    s.add_argument("--repo")
    s.add_argument("--commons")
    s.add_argument("--registry")

    s = sub.add_parser("export", help="copy the store to a .sql or .sqlite file outside every project")
    s.add_argument("--sql")
    s.add_argument("--db")
    s.add_argument("--force", action="store_true")

    sub.add_parser("schema", help="print the schema")

    s = sub.add_parser("records", help="the tally as typed records (what the dashboard reads)")
    s.add_argument("--project")
    s.add_argument("--area")
    s.add_argument("--now")
    s.add_argument("--quorum", type=int)
    s.add_argument("--expiry-days", type=int)
    s.add_argument("--stale-days", type=int)
    s.add_argument("--amend-quorum", type=int)
    s.add_argument("--amend-majority")

    sub.add_parser("status", help="one screen: store, projects, agents, jobs, runner")
    s = sub.add_parser("events", help="the append-only event log")
    s.add_argument("--limit", type=int, default=40)
    return p


def print_jobs(con, project=None, include_finished=False):
    sql = "SELECT * FROM jobs"
    conds, args = [], []
    if project:
        conds.append("project = ?")
        args.append(project)
    if not include_finished:
        conds.append("status IN ('queued', 'leased', 'blocked')")
    if conds:
        sql += " WHERE " + " AND ".join(conds)
    sql += " ORDER BY status, priority DESC, created_at"
    rows = con.execute(sql, args).fetchall()
    if not rows:
        print("no jobs")
    for j in rows:
        lease = " by %s until %s" % (j["lease_agent"], j["lease_until"]) if j["status"] == "leased" else ""
        print("%-9s %-14s %-8s tier %d  %-34s %s%s" % (j["status"], j["project"], j["kind"], j["tier"], j["id"], trunc(j["title"], 60), lease))
        if j["result"]:
            print("          %s" % trunc(j["result"], 100))


def main(argv=None):
    args = build_parser().parse_args(argv)
    home = args.home
    con = None
    try:
        if args.cmd == "init":
            con = connect(home, create=True)
            proj, created = add_project(con, args.path, name=args.name, remote=args.remote, brain=args.brain,
                                        verify_cmd=args.verify, actor=args.agent or os.environ.get("PULSE_AGENT", "") or HUMAN_AGENT)
            job, new = add_job(con, proj["slug"], "onboard", "Onboard %s: read AGENTS.md, run the checks, post the first findings" % proj["slug"],
                               "Read the project's AGENTS.md, docs/context/, docs/memory/, and open decisions. Run its checks. Write FINDING: lines "
                               "for what the next agent would rediscover and SUMMARY: with the project's state. Change nothing unless a check "
                               "proves a Tier 0 fix.", tier=0, priority=20, source="onboard:%s" % proj["slug"], created_by="init")
            print("%s project %s at %s%s" % ("registered" if created else "refreshed", proj["slug"], proj["path"], " (brain)" if proj["brain"] else ""))
            print("%s onboarding job %s" % ("queued" if new else "already queued", job["id"]))
            print("store: %s" % store_path(home))
            return 0
        if args.cmd == "schema":
            print(schema_text(), end="")
            return 0
        con = connect(home)
        if args.cmd == "agent":
            if args.sub == "join":
                row, created = join_agent(con, args.name, args.runner, args.model, args.session, args.display, args.kind)
                print("%s %s (%s/%s); export PULSE_AGENT=%s" % ("joined" if created else "welcome back", row["name"], row["runner"], row["model"], row["name"]))
            elif args.sub == "spawn":
                row = spawn_agent(con, args.runner, args.model)
                print(row["name"])
            elif args.sub == "list":
                for r in agent_scorecards(con, include_pruned=args.all):
                    print("%-16s %-7s %-8s posts %-4d jobs %d/%d  last useful %s  %s/%s" % (
                        r["name"], r["status"], r["kind"], r["posts"], r["jobs_done"], r["jobs_done"] + r["jobs_failed"],
                        r["last_useful_at"] or "-", r["runner"], r["model"]))
            elif args.sub == "prune":
                for name, reason in prune_agents(con, now=args.now, days=args.days, dry_run=args.dry_run):
                    print("%s%s: %s" % ("would prune " if args.dry_run else "pruned ", name, reason))
            return 0
        if args.cmd == "agents":
            for r in agent_scorecards(con, include_pruned=args.all):
                print("%-16s %-7s %-8s posts %-4d findings %-3d suggestions %-3d votes %-3d jobs %d/%d  last useful %s" % (
                    r["name"], r["status"], r["kind"], r["posts"], r["findings"], r["suggestions"], r["votes"], r["jobs_done"],
                    r["jobs_done"] + r["jobs_failed"], r["last_useful_at"] or "-"))
            return 0
        if args.cmd == "post":
            proj = current_project(con, args.project)
            expires = args.expires or ""
            if args.kind == "claim" and not expires:
                expires = plus_hours(utc_now(), args.claim_hours if args.claim_hours else float(setting(con, "claim_hours")))
            row = create_post(con, args.kind, proj["slug"], args.area, args.thread, args.summary, read_body(args), agent=args.agent,
                              session=args.session or "", on=args.on, vote=args.vote, expires=expires, resolution=args.resolution,
                              links=args.links, files=args.files, text_hash=args.text_hash, origin_project=args.origin_project)
            print(row["id"])
            return 0
        if args.cmd == "claim":
            proj = current_project(con, args.project)
            expires = plus_hours(utc_now(), args.hours if args.hours else float(setting(con, "claim_hours")))
            row = create_post(con, "claim", proj["slug"], args.area, args.thread, args.summary, "", agent=args.agent, expires=expires)
            print(row["id"])
            return 0
        if args.cmd == "vote":
            target = post_row(con, args.on)
            if target is None:
                raise PulseError("No post %r" % args.on, 2)
            row = create_post(con, "vote", target["project"], target["area"], target["thread"], args.summary, args.body, agent=args.agent,
                              on=args.on, vote=args.vote, text_hash=args.text_hash)
            print(row["id"])
            return 0
        if args.cmd == "board":
            proj = current_project(con, args.project)
            agent = resolve_agent(con, args.agent, required=False)
            print(digest(con, proj["slug"], args.area, args.now, agent["name"] if agent else None), end="")
            return 0
        if args.cmd == "show":
            print(show_post(con, args.post_id), end="")
            return 0
        if args.cmd == "queue":
            proj = current_project(con, args.project)
            t, params = tally_for(con, proj["slug"], None, args.now)
            for s in t.suggestion_records():
                if s["status"] == "closed":
                    continue
                print("%+3d  %-12s %-38s %-44s up %d down %d voters %d  %s" % (s["score"], s["status"], s["key"], trunc(s["id"], 44), s["up"], s["down"], s["voters"], trunc(s["summary"], 50)))
            return 0
        if args.cmd == "ballot":
            proj = current_project(con, args.project)
            agent = resolve_agent(con, args.agent, required=False)
            sug, am = ballot_items(con, proj["slug"], agent["name"] if agent else None, args.now)
            print("BALLOT for %s in %s (vote with: pulse vote POST_ID up|down --summary REASON)" % (agent["name"] if agent else "everyone", proj["slug"]))
            for s in sug:
                print("  suggestion  %-12s %-38s %-44s up %d down %d voters %d  %s" % (s["status"], s["key"], trunc(s["id"], 44), s["up"], s["down"], s["voters"], trunc(s["summary"], 50)))
            for a in am:
                print("  amendment   %-12s %-38s %-44s up %d down %d voters %d/%d  hash %s  %s" % (trunc(a["status"], 12), a["key"], trunc(a["id"], 44), a["up"], a["down"], a["voters"], a["quorum"], a["hash"][:12], trunc(a["summary"], 40)))
            if not sug and not am:
                print("  nothing to vote on")
            return 0
        if args.cmd == "proposals":
            slugs = [args.project] if args.project else [p["slug"] for p in project_rows(con)]
            n = 0
            for slug in slugs:
                t, _ = tally_for(con, slug)
                for key, pr in t.prop.items():
                    st = t.proposal_status(key)
                    print("%-10s %-14s %-38s %s  %s" % (st, slug, key, pr["id"], trunc(pr["summary"], 70)))
                    if st == "awaiting":
                        print("           decide: pulse decide %s yes|no --words '...'" % pr["id"])
                    n += 1
            if not n:
                print("no proposals")
            return 0
        if args.cmd == "decide":
            row = decide(con, args.post_id, args.answer, args.words)
            print("recorded %s as %s" % (args.answer, row["id"]))
            return 0
        if args.cmd == "hash":
            row = post_row(con, args.post_id)
            if row is None:
                raise PulseError("No post %r" % args.post_id, 2)
            block = fenced_block(row["body"], "Proposed text")
            if block is None:
                raise PulseError("No fenced block under '## Proposed text' in %s" % args.post_id)
            print(block_hash(block))
            return 0
        if args.cmd == "projects":
            for p in project_rows(con, include_inactive=True):
                print("%-18s %-8s %s%s  %s" % (p["slug"], p["status"], p["path"], "  (brain)" if p["brain"] else "", p["remote"]))
            return 0
        if args.cmd == "jobs":
            print_jobs(con, args.project, args.all)
            return 0
        if args.cmd == "job":
            if args.sub == "add":
                proj = current_project(con, args.project)
                job, new = add_job(con, proj["slug"], args.kind, args.title, args.brief, args.tier, args.priority, args.source,
                                   created_by=args.agent or os.environ.get("PULSE_AGENT", ""))
                print(job["id"] if new else "already queued: %s" % job["id"])
            else:
                agent = resolve_agent(con, args.agent, required=False)
                job = finish_job(con, args.job_id, agent["name"] if agent else HUMAN_AGENT, args.status, args.result)
                print("%s %s" % (job["id"], job["status"]))
            return 0
        if args.cmd == "plan":
            slugs = [current_project(con, args.project)["slug"]] if args.project else [p["slug"] for p in project_rows(con)]
            for slug in slugs:
                for job in plan_jobs(con, slug, args.now, created_by="plan"):
                    print("planned %s %s: %s" % (job["kind"], job["id"], job["title"]))
            return 0
        if args.cmd == "work":
            worker = pick_worker(con, args.agent)
            run, reason = work_once(con, worker, project=args.project, job_id=args.job, dry_run=args.dry_run,
                                    worker_cmd=args.worker_cmd, home=home, now=args.now)
            if run is None:
                print("idle: %s" % reason)
                return 3
            print("%s %s by %s: %s" % (run["status"], run["job_id"], run["agent"], run["summary"]))
            if run["branch"]:
                print("branch %s (local; review and merge by hand)" % run["branch"])
            return 0 if run["status"] in ("done", "dry-run") else 1
        if args.cmd == "daemon":
            result = daemon(con, args.agent, args.interval, args.dry_run, args.worker_cmd, home, args.max_loops)
            return 0 if result == "done" else 3
        if args.cmd in ("stop", "start"):
            set_setting(con, "stop", "1" if args.cmd == "stop" else "0", os.environ.get("PULSE_AGENT", "") or HUMAN_AGENT)
            print("runner %s" % ("stopped" if args.cmd == "stop" else "allowed"))
            return 0
        if args.cmd == "settings":
            if args.action == "list" or (args.action == "get" and not args.name):
                for r in con.execute("SELECT * FROM settings ORDER BY name"):
                    print("%-18s %-40s %s %s" % (r["name"], r["value"], r["updated_at"], r["updated_by"]))
            elif args.action == "get":
                print(setting(con, args.name))
            else:
                if args.value is None:
                    raise PulseError("settings set NAME VALUE", 2)
                set_setting(con, args.name, args.value, args.agent or os.environ.get("PULSE_AGENT", "") or HUMAN_AGENT)
                print("%s=%s" % (args.name, args.value))
            return 0
        if args.cmd == "migrate":
            report = migrate(con, args.repo, args.commons, args.registry)
            for slug, created in report["registry"]:
                print("registry: %s %s" % ("added" if created else "kept", slug))
            if "board" in report:
                slug, counts = report["board"]
                print("board %s: %d posts imported, %d already there, %d quarantined, %d agents named" % (slug, counts["posts"], counts["skipped"], counts["quarantined"], counts["agents"]))
            if "commons" in report:
                path, counts = report["commons"]
                print("commons %s: %d posts imported, %d already there, %d quarantined, %d agents named" % (path, counts["posts"], counts["skipped"], counts["quarantined"], counts["agents"]))
            return 0
        if args.cmd == "export":
            if not args.sql and not args.db:
                raise PulseError("export --sql PATH or --db PATH", 2)
            print(export_store(con, args.sql, args.db, args.force))
            return 0
        if args.cmd == "records":
            proj = current_project(con, args.project)
            flags = {k: getattr(args, k) for k in ("quorum", "expiry_days", "stale_days", "amend_quorum", "amend_majority")}
            sys.stdout.write(snapshot_records(con, proj, args.area, args.now, **flags))
            return 0
        if args.cmd == "status":
            print(status_text(con), end="")
            return 0
        if args.cmd == "events":
            for r in con.execute("SELECT * FROM events ORDER BY seq DESC LIMIT ?", (args.limit,)):
                print("%s %-8d %-18s %-16s %-40s %s" % (r["at"], r["seq"], r["action"], trunc(r["actor"], 16), trunc(r["subject"], 40), trunc(r["detail"], 80)))
            return 0
    except PulseError as exc:
        print("pulse: %s" % exc, file=sys.stderr)
        return exc.code
    except sqlite3.IntegrityError as exc:
        print("pulse: %s" % exc, file=sys.stderr)
        return 1
    finally:
        if con is not None:
            con.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
