#!/usr/bin/env python3
"""Tests for docs/scripts/pulse-dashboard.py (ADR-015, FEAT-009).

Run from the repository root:

    python3 -m unittest discover -s docs/scripts/tests -p 'test_*dashboard*.py'

Every test builds a throwaway PULSE repository in a temporary directory, so
nothing here reads or writes the real board. The fixture holds one of each
board situation the dashboard must show faithfully: an eligible suggestion
with counted, self, latest-wins and down votes; a suggestion still needing
votes; an expired one; an active and an expired claim; a closed thread; an
amendment with counted, self, hash-mismatch and no-Read-line votes; and a
post whose text is an HTML/JS payload that must stay inert.
"""

import importlib.util
import io
import json
import os
import shutil
import stat
import subprocess
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
BOARD_SH = SCRIPTS / "board.sh"
EVOLUTION_SH = SCRIPTS / "evolution-report.sh"
NOW = "2026-09-10T12:00:00Z"


def load_dashboard():
    spec = importlib.util.spec_from_file_location("pulse_dashboard", SCRIPTS / "pulse-dashboard.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


dash = load_dashboard()

# Every test in this module runs with the user-level PULSE home pinned to a
# throwaway directory. `index` and `serve` register their repository there by
# default (ADR-016), so without this guard a test that forgets --no-register
# would write to the real ~/.pulse/projects.json.
MODULE_HOME = None
_ENV_BEFORE = {}


def setUpModule():
    global MODULE_HOME
    MODULE_HOME = Path(tempfile.mkdtemp(prefix="pulse-dashboard-module-home-")).resolve()
    for key in ("PULSE_HOME", "PULSE_COMMONS", "PULSE_DASHBOARD_HOME"):
        _ENV_BEFORE[key] = os.environ.pop(key, None)
    os.environ["PULSE_HOME"] = str(MODULE_HOME)


def tearDownModule():
    for key, value in _ENV_BEFORE.items():
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value
    if MODULE_HOME is not None:
        shutil.rmtree(MODULE_HOME, ignore_errors=True)


def registry_slugs(home):
    path = Path(home) / dash.REGISTRY_NAME
    if not path.exists():
        return []
    return [p["slug"] for p in json.loads(path.read_text())["projects"]]

PAYLOAD = "<script>alert(1)</script><img src=x onerror=\"alert(2)\"> & \"quotes\" 'single'"

CONSTITUTION = """---
loop: off
limits: none
---

# Constitution (test fixture)

## Hard stops

Never cross a hard stop.

## Immutable paths

- `docs/evolution/constitution.md`

## Editable scope by stage

| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --- | --- | --- | --- | --- | --- | --- |
| product code | `src/` | 4 | human | 5 | 0 | 0 |
| memory | `docs/memory/` | 1 | human | 5 | 0 | 0 |

## Vote rules

| Rule | Value |
| --- | --- |
| Quorum to enter the queue | 2 distinct agent identities |
| Votes per identity per suggestion | 1 (the latest wins) |
| Amendment voter | one top-level session |
| Informed vote | text_hash plus a Read: line |
| Amendment quorum | 1 distinct voter |
| Amendment supermajority | 2/3 of counted votes |
| Not amendable by vote | Hard stops, Kill switch, Amendments, frontmatter |

## Amendments

Per docs/workflows/amend.md.

## Kill switch

A file named docs/evolution/STOP halts every cycle.
"""

EVALUATORS = """---
baseline: B-001
---

# Evaluators (test fixture)

## Constraints (pass/fail)

- C1 the test suite passes

## Product evaluators

- P1 unit tests pass
"""

ARCHIVE = """# Archive (test fixture)

## Baselines

| Row | Date | Tree | Constraints | S1 | Notes |
| --- | --- | --- | --- | --- | --- |
| B-001 | 2026-09-01 | v1 (abc1234) | C1 pass | 0.5 | first baseline |

## Variants

| Row | Date | Parent | Suggestion | Votes | File class | Change (one variable) | Hypothesis | Target: before -> after | Other scores | Decision | Ref |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| A-001 | 2026-09-02 | B-001 | 20260901T090000Z-agent-a | +2 | product code | tighten the parser | fewer retries | S1 0.5 -> 0.7 | - | kept | PR-1 |
| A-002 | 2026-09-03 | B-001 | - | - | memory | drop a stale lesson | clearer memory | S1 0.7 -> 0.6 | - | discarded | - |

## Incidents

| Row | Date | File class | What happened | Variant | Action |
| --- | --- | --- | --- | --- | --- |
"""


def post_text(kind, area, thread, agent, date, summary, session=None, body="", **extra):
    runner, _, model = agent.partition("/")
    lines = [
        "---",
        "kind: %s" % kind,
        "area: %s" % area,
        "thread: %s" % thread,
        "agent: %s" % agent,
        "runner: %s" % runner,
        "model: %s" % model,
    ]
    if session is not None:
        lines.append("session: %s" % session)
    lines += [
        "date: %s" % date,
        "branch: main",
        "links: []",
        "summary: %s" % summary,
    ]
    for key, value in extra.items():
        lines.append("%s: %s" % (key, value))
    lines += ["---", "", "## %s" % kind, "", body or summary, ""]
    return "\n".join(lines)


def post_id(date, agent):
    return date.replace("-", "").replace(":", "") + "-" + agent.replace("/", "_")


class Fixture:
    """A throwaway PULSE repository with one of every board situation."""

    def __init__(self, root, name="fixture-repo"):
        self.repo = Path(root) / name
        self.ids = {}
        self.amendment_hash = ""
        self._write_control_plane()
        self._write_board()
        self._git_init()

    def _write(self, rel, text):
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def post(self, kind, area, thread, agent, date, summary, **kw):
        pid = post_id(date, agent)
        self._write("docs/board/%s/%s/%s.md" % (area, thread, pid),
                    post_text(kind, area, thread, agent, date, summary, **kw))
        return pid

    def _write_control_plane(self):
        self._write("docs/evolution/constitution.md", CONSTITUTION)
        self._write("docs/evolution/evaluators.md", EVALUATORS)
        self._write("docs/evolution/archive.md", ARCHIVE)
        self._write("docs/board/README.md", "# Board (fixture)\n")

    def _write_board(self):
        a, b, c, d = "runner-a/model-a", "runner-b/model-b", "runner-c/model-c", "runner-d/model-d"
        # Eligible suggestion: B up, C up then down (latest wins), D up, A self-up (skipped).
        s1 = self.post("suggestion", "evolution", "speedup", a, "2026-09-01T09:00:00Z",
                       "Cache the parser table", session="s-a")
        self.post("vote", "evolution", "speedup", b, "2026-09-02T09:00:00Z", "up: fewer retries", session="s-b", on=s1, vote="up")
        self.post("vote", "evolution", "speedup", c, "2026-09-02T10:00:00Z", "up at first", session="s-c", on=s1, vote="up")
        self.post("vote", "evolution", "speedup", c, "2026-09-03T10:00:00Z", "down after reading the archive", session="s-c", on=s1, vote="down")
        self.post("vote", "evolution", "speedup", d, "2026-09-03T11:00:00Z", "up", session="s-d", on=s1, vote="up")
        self.post("vote", "evolution", "speedup", a, "2026-09-03T12:00:00Z", "my own", session="s-a", on=s1, vote="up")
        self.ids["eligible"] = s1
        # Suggestion with one vote: needs votes.
        s3 = self.post("suggestion", "evolution", "fresh", b, "2026-09-08T09:00:00Z", "Split the long fixture", session="s-b")
        self.post("vote", "evolution", "fresh", c, "2026-09-09T09:00:00Z", "up", session="s-c", on=s3, vote="up")
        self.ids["needs_votes"] = s3
        # Unvoted suggestion older than the expiry window: expired.
        self.ids["expired"] = self.post("suggestion", "evolution", "lonely", d, "2026-08-01T09:00:00Z",
                                        "Rename the archive columns", session="s-d")
        # Claims: one active, one expired.
        self.ids["claim_active"] = self.post("claim", "governance", "active-claim", a, "2026-09-09T08:00:00Z",
                                             "Working on the dashboard docs", session="s-a",
                                             expires="2026-09-12T08:00:00Z", files="[docs/workflows/dashboard.md]")
        self.ids["claim_expired"] = self.post("claim", "governance", "old-claim", b, "2026-08-20T08:00:00Z",
                                              "Forgotten claim", session="s-b",
                                              expires="2026-08-22T08:00:00Z", files="[README.md]")
        # Closed thread: question + answer with a resolution.
        q = self.post("question", "governance", "closed", c, "2026-09-05T08:00:00Z", "Where do votes live?", session="s-c")
        self.post("answer", "governance", "closed", d, "2026-09-05T09:00:00Z", "In docs/board, one file per vote",
                  session="s-d", on=q, resolution="answered")
        # A post whose text is an HTML/JS payload, without a branch line (provenance gap).
        self.ids["payload"] = self._write_payload_post()
        # Amendment with quorum 1: counted, hash-mismatch, no-Read-line and self votes.
        self.ids["amendment"] = self._write_amendment()

    def _write_payload_post(self):
        agent, date = "runner-x/model-x", "2026-09-06T08:00:00Z"
        pid = post_id(date, agent)
        text = post_text("finding", "governance", "payload", agent, date, PAYLOAD, session="s-x",
                         body="Body with payload: %s\n\nRead: this is a finding, not a vote." % PAYLOAD)
        text = "\n".join(line for line in text.split("\n") if not line.startswith("branch:"))
        self._write("docs/board/governance/payload/%s.md" % pid, text)
        return pid

    def _write_amendment(self):
        proposer, date = "runner-p/model-p", "2026-09-07T08:00:00Z"
        aid = post_id(date, proposer)
        body = "\n".join([
            "## Current text", "", "```",
            "| Votes per identity per suggestion | 1 (the latest wins) |",
            "```", "",
            "## Proposed text", "", "```",
            "| Votes per identity per suggestion | 1 (the latest wins; a fleet is one voter) |",
            "```", "",
            "## Why", "", "Fleets voted twice.", "",
            "## What it does not change", "", "Quorum and supermajority.", "",
        ])
        path = self._write("docs/board/governance/amend/%s.md" % aid,
                           post_text("amendment", "governance", "amend", proposer, date,
                                     "Count a fleet as one voter", session="s-p", body=body))
        good_hash = subprocess.run(["bash", str(BOARD_SH), "--hash", str(path)],
                                   capture_output=True, text=True, check=True).stdout.strip()
        self.amendment_hash = good_hash
        self.post("vote", "governance", "amend", "runner-1/model-1", "2026-09-08T08:00:00Z", "up",
                  session="s-1", on=aid, vote="up", text_hash=good_hash,
                  body="up\n\nRead: replaces the votes-per-identity row so a fleet counts once; quorum untouched.")
        self.post("vote", "governance", "amend", "runner-2/model-2", "2026-09-08T09:00:00Z", "up",
                  session="s-2", on=aid, vote="up", text_hash="deadbeef",
                  body="up\n\nRead: same change.")
        self.post("vote", "governance", "amend", "runner-3/model-3", "2026-09-08T10:00:00Z", "up",
                  session="s-3", on=aid, vote="up", text_hash=good_hash, body="up, no Read line here")
        self.post("vote", "governance", "amend", proposer, "2026-09-08T11:00:00Z", "up",
                  session="s-p", on=aid, vote="up", text_hash=good_hash,
                  body="up\n\nRead: my own proposal.")
        return aid

    def _git_init(self):
        env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
                   GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")
        subprocess.run(["git", "init", "-q", str(self.repo)], env=env, check=True, capture_output=True)
        for cmd in (["git", "checkout", "-q", "-B", "main"], ["git", "add", "-A"],
                    ["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture"]):
            subprocess.run(cmd, cwd=str(self.repo), env=env, check=True, capture_output=True)

    def git_status(self):
        return subprocess.run(["git", "status", "--porcelain"], cwd=str(self.repo),
                              capture_output=True, text=True, check=True).stdout

    def board_text(self, *args):
        return subprocess.run(["bash", str(BOARD_SH), "--repo", str(self.repo), "--now", NOW] + list(args),
                              capture_output=True, text=True, check=True).stdout


def query_db(db, fn, *args):
    con = dash.open_db(db)
    try:
        return fn(con, *args)
    finally:
        con.close()


def count(db, table):
    return query_db(db, lambda con: con.execute("SELECT COUNT(*) FROM %s" % table).fetchone()[0])


class ProjectionTestCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-test-")).resolve()
        cls.fx = Fixture(cls.tmp)
        cls.home = cls.tmp / "state-home"
        cls.state_dir = dash.project_state_dir(cls.fx.repo, cls.home)
        cls.db = dash.build_projection(cls.fx.repo, cls.state_dir, now=NOW)

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def query(self, fn, *args):
        return query_db(self.db, fn, *args)

    # --- projection content ------------------------------------------------

    def test_overview_counts_every_board_situation(self):
        o = self.query(dash.q_overview)
        self.assertEqual(o["claims_active"], 1)
        self.assertEqual(o["claims_expired"], 1)
        self.assertEqual(o["threads_closed"], 1)
        self.assertEqual(o["suggestions_eligible"], 1)
        self.assertEqual(o["suggestions_needing_votes"], 1)
        self.assertEqual(o["suggestions_expired"], 1)
        self.assertEqual(o["amendments_open"], 1)
        self.assertEqual(o["amendments_passed"], 1)
        self.assertEqual(o["posts_without_provenance"], 1)
        self.assertEqual(o["posts"], 19)
        self.assertTrue(o["board_present"])
        self.assertEqual(o["quorum"], 2)
        self.assertEqual(o["active_claims"][0]["id"], self.fx.ids["claim_active"])
        self.assertEqual(o["stale_claims"][0]["id"], self.fx.ids["claim_expired"])
        self.assertEqual(o["evolution"]["loop"], "off")
        self.assertEqual(o["evolution"]["baseline"], "B-001")

    def test_suggestion_tally_matches_board_sh(self):
        suggestions = self.query(dash.q_suggestions)
        by_id = {s["id"]: s for s in suggestions}
        s1 = by_id[self.fx.ids["eligible"]]
        self.assertEqual((s1["up"], s1["down"], s1["voters"], s1["score"], s1["status"], s1["rank"]),
                         (2, 1, 3, 1, "eligible", 1))
        votes = {v["agent"]: v for v in s1["votes"]}
        self.assertEqual(votes["runner-c/model-c"]["vote"], "down", "the latest vote per identity wins")
        self.assertEqual(votes["runner-c/model-c"]["classification"], "counted")
        self.assertEqual(votes["runner-a/model-a"]["classification"], "self")
        self.assertTrue(all(v["reason"] for v in s1["votes"]), "each vote carries its one-line reason")
        self.assertEqual(by_id[self.fx.ids["needs_votes"]]["status"], "needs votes")
        self.assertEqual(by_id[self.fx.ids["expired"]]["status"], "expired")
        # The same numbers the text report prints (the only tally).
        queue = self.fx.board_text("--queue")
        line = next(l for l in queue.splitlines() if self.fx.ids["eligible"] in l)
        self.assertIn("up 2 down 1 voters 3", line)
        self.assertIn("eligible", line)
        self.assertTrue(line.strip().startswith("+1"), line)
        self.assertIn("expired", next(l for l in queue.splitlines() if self.fx.ids["expired"] in l))

    def test_amendment_tally_matches_board_sh(self):
        amendments = self.query(dash.q_amendments)
        self.assertEqual(len(amendments), 1)
        a = amendments[0]
        self.assertEqual(a["id"], self.fx.ids["amendment"])
        self.assertTrue(a["status"].startswith("passed"), a["status"])
        self.assertEqual((a["up"], a["down"], a["voters"], a["quorum"]), (1, 0, 1, 1))
        self.assertEqual((a["uncounted_self"], a["uncounted_hash_mismatch"], a["uncounted_no_read_line"]), (1, 1, 1))
        self.assertEqual(a["text_hash"], self.fx.amendment_hash)
        classes = {v["session"]: v["classification"] for v in a["votes"]}
        self.assertEqual(classes, {"s-1": "counted", "s-2": "hash mismatch", "s-3": "no Read line", "s-p": "self"})
        read_lines = {v["session"]: v["read_line"] for v in a["votes"]}
        self.assertTrue(read_lines["s-1"].startswith("replaces the votes-per-identity row"))
        self.assertEqual(read_lines["s-3"], "", "a vote without a Read: line shows an empty restatement")
        text = self.fx.board_text("--amendments")
        self.assertIn("up 1 down 0  voters 1/1  100% up  identities 1  uncounted: 1 self, 1 hash mismatch, 1 no Read line", text)
        self.assertIn("passed", text)

    def test_thread_and_post_views(self):
        threads = {t["key"]: t for t in self.query(dash.q_threads)}
        self.assertEqual(threads["governance/closed"]["status"], "closed")
        self.assertEqual(threads["governance/closed"]["resolution"], "answered")
        self.assertEqual(threads["governance/active-claim"]["status"], "open")
        detail = self.query(dash.q_thread, "evolution/speedup")
        self.assertEqual(len(detail["posts"]), 6)
        self.assertEqual(detail["posts"][0]["kind"], "suggestion")
        post = self.query(dash.q_post, self.fx.ids["eligible"])
        self.assertEqual(len(post["responses"]), 5)
        self.assertEqual(post["frontmatter"]["kind"], "suggestion")
        payload = self.query(dash.q_post, self.fx.ids["payload"])
        self.assertEqual(payload["summary"], PAYLOAD, "text is stored verbatim; the page renders it as text")
        self.assertEqual(payload["provenance_ok"], 0)
        self.assertEqual(payload["provenance_missing"], ["branch"])
        self.assertIsNone(self.query(dash.q_post, "no-such-post"))
        self.assertIsNone(self.query(dash.q_thread, "no/such-thread"))

    def test_evolution_view(self):
        e = self.query(dash.q_evolution)
        self.assertEqual(e["summary"]["loop"], "off")
        self.assertEqual(e["summary"]["limits"], "none")
        self.assertEqual(e["summary"]["baseline"], "B-001")
        self.assertEqual(e["summary"]["stop_present"], "0")
        self.assertEqual(sorted(e["archive"]), ["Baselines", "Incidents", "Variants"])
        self.assertEqual([r["id"] for r in e["archive"]["Variants"]["rows"]], ["A-001", "A-002"])
        self.assertEqual(e["archive"]["Variants"]["columns"][0], "Row")
        self.assertEqual(e["decisions"], {"discarded": 1, "kept": 1})
        self.assertEqual(e["origins"], {"voted": {"tried": 1, "kept": 1}, "signal": {"tried": 1, "kept": 0}})
        self.assertEqual([s["file_class"] for s in e["scope"]], ["product code", "memory"])
        self.assertEqual(e["scope"][0]["stage"], "4")
        self.assertEqual(e["summary"]["product_classes_ok"], "1")
        self.assertEqual(e["summary"]["product_evaluators_ok"], "1")
        self.assertEqual(e["hygiene"], [])

    def test_project_metadata(self):
        meta = self.query(dash.q_project)
        self.assertEqual(meta["schema_version"], str(dash.SCHEMA_VERSION))
        self.assertEqual(meta["branch"], "main")
        self.assertEqual(meta["dirty"], "0")
        self.assertEqual(meta["board_params"]["amend_quorum"], 1)
        self.assertEqual(meta["board_params"]["quorum"], 2)
        self.assertEqual(meta["board_params"]["now"], NOW)
        self.assertEqual(meta["tally_script"], str(BOARD_SH))

    # --- placement, isolation, reproducibility ----------------------------

    def test_projection_lives_outside_the_repository(self):
        self.assertFalse(dash.is_inside(self.db, self.fx.repo))
        self.assertEqual(self.fx.git_status(), "", "building the projection must not touch the repository")
        with self.assertRaises(dash.DashboardError):
            dash.project_state_dir(self.fx.repo, self.fx.repo / ".cache")
        if os.name != "nt":
            self.assertEqual(stat.S_IMODE(self.db.stat().st_mode), 0o600, "the projection is owner-only")
            self.assertEqual(stat.S_IMODE(self.state_dir.stat().st_mode), 0o700, "the state dir is owner-only")

    def test_two_repositories_get_separate_projections(self):
        other = Fixture(self.tmp, name="other-repo")
        other_dir = dash.project_state_dir(other.repo, self.home)
        self.assertNotEqual(other_dir, self.state_dir)
        self.assertEqual(other_dir.parent, self.state_dir.parent)
        db2 = dash.build_projection(other.repo, other_dir, now=NOW)
        self.assertTrue(db2.is_file() and self.db.is_file())
        self.assertEqual(query_db(db2, dash.q_project)["repo"], str(other.repo))

    def test_rebuild_after_delete_and_refresh_after_new_post(self):
        fx = Fixture(self.tmp, name="rebuild-repo")
        state_dir = dash.project_state_dir(fx.repo, self.home)
        db = dash.build_projection(fx.repo, state_dir, now=NOW)
        before = dash.read_metadata(db)
        first = (count(db, "posts"), count(db, "threads"))
        db.unlink()
        self.assertIsNone(dash.read_metadata(db))
        dash.build_projection(fx.repo, state_dir, now=NOW)
        self.assertEqual((count(db, "posts"), count(db, "threads")), first)
        self.assertEqual(dash.read_metadata(db)["fingerprint"], before["fingerprint"])
        self.assertTrue(dash.projection_is_current(fx.repo, db)[0])
        fx.post("finding", "governance", "late", "runner-z/model-z", "2026-09-09T20:00:00Z", "A new post", session="s-z")
        self.assertFalse(dash.projection_is_current(fx.repo, db)[0], "a new post changes the fingerprint")
        state = dash.DashboardState(fx.repo, state_dir, refresh_seconds=0)
        state.ensure_fresh()
        self.assertEqual((count(db, "posts"), count(db, "threads")), (first[0] + 1, first[1] + 1))
        self.assertTrue(dash.projection_is_current(fx.repo, db)[0])
        self.assertEqual(dash.read_metadata(db)["dirty"], "1", "an uncommitted post marks the projection dirty")

    # --- the shell entry points -------------------------------------------

    def test_board_sh_json_and_records_flags(self):
        snap = json.loads(self.fx.board_text("--json"))
        self.assertEqual(snap["schema"], dash.BOARD_SNAPSHOT_SCHEMA)
        self.assertEqual(len(snap["posts"]), 19)
        self.assertEqual(snap["params"]["amend_quorum"], 1)
        evo = json.loads(subprocess.run(["bash", str(EVOLUTION_SH), "--repo", str(self.fx.repo), "--json"],
                                        capture_output=True, text=True, check=True).stdout)
        self.assertEqual(evo["schema"], dash.EVOLUTION_SNAPSHOT_SCHEMA)
        self.assertEqual(evo["loop"], "off")
        records = self.fx.board_text("--records")
        kinds = {line.split("\t")[0] for line in records.splitlines()}
        self.assertTrue({"param", "post", "claim", "thread", "suggestion", "svote", "amendment", "avote"} <= kinds, kinds)
        for bad in (["--records", "--tsv"], ["--records", "--queue"], ["--json", "--claims"]):
            proc = subprocess.run(["bash", str(BOARD_SH), "--repo", str(self.fx.repo)] + bad, capture_output=True, text=True)
            self.assertEqual(proc.returncode, 2, bad)

    def test_cli_index_status_and_snapshot(self):
        real_home = Path.home() / ".pulse"
        real_before = (real_home / dash.REGISTRY_NAME).read_bytes() if (real_home / dash.REGISTRY_NAME).exists() else None
        out, _ = self.run_main(["index", "--repo", str(self.fx.repo), "--state-dir", str(self.home), "--now", NOW])
        self.assertIn("posts 19", out)
        self.assertIn("Registered: /p/fixture-repo/", out, "index registers its repository by default")
        self.assertEqual(registry_slugs(MODULE_HOME), ["fixture-repo"], "into the module's throwaway home")
        real_after = (real_home / dash.REGISTRY_NAME).read_bytes() if (real_home / dash.REGISTRY_NAME).exists() else None
        self.assertEqual(real_before, real_after, "the tests never touch the real ~/.pulse")
        out, _ = self.run_main(["status", "--repo", str(self.fx.repo), "--state-dir", str(self.home)])
        self.assertIn("Fresh:       yes", out)
        self.assertIn(str(self.state_dir), out)
        self.assertIn("Rows:        posts 19", out)
        self.assertIn("Schema:      %d" % dash.SCHEMA_VERSION, out)
        out, _ = self.run_main(["snapshot", "--repo", str(self.fx.repo), "--now", NOW])
        payload = json.loads(out)
        self.assertEqual(payload["board"]["schema"], dash.BOARD_SNAPSHOT_SCHEMA)
        self.assertEqual(payload["evolution"]["schema"], dash.EVOLUTION_SNAPSHOT_SCHEMA)
        self.assertEqual(self.fx.git_status(), "")
        _, err = self.run_main(["index", "--repo", str(self.tmp)], expect=1)
        self.assertIn("No docs/ control plane", err)

    def run_main(self, argv, expect=0):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = dash.main(argv)
        self.assertEqual(rc, expect, err.getvalue())
        return out.getvalue(), err.getvalue()


class ServerTestCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-server-")).resolve()
        cls.fx = Fixture(cls.tmp)
        cls.state_dir = dash.project_state_dir(cls.fx.repo, cls.tmp / "home")
        cls.state = dash.DashboardState(cls.fx.repo, cls.state_dir, refresh_seconds=3600)
        cls.state.ensure_fresh(force=True)
        cls.server = dash.DashboardServer(cls.state, port=0)
        cls.thread = threading.Thread(target=cls.server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True)
        cls.thread.start()
        cls.url = cls.server.url.rstrip("/")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def request(self, path, method="GET"):
        req = urllib.request.Request(self.url + path, method=method)
        try:
            with urllib.request.urlopen(req, timeout=10) as resp:
                return resp.status, dict(resp.headers), resp.read()
        except urllib.error.HTTPError as exc:
            with exc:
                return exc.code, dict(exc.headers), exc.read()

    def test_binds_loopback_only(self):
        self.assertEqual(self.server.server_address[0], "127.0.0.1")
        self.assertIn("127.0.0.1", self.url)

    def test_security_headers_and_static_assets(self):
        status, headers, body = self.request("/")
        self.assertEqual(status, 200)
        self.assertIn("text/html", headers["Content-Type"])
        self.assertEqual(headers["X-Content-Type-Options"], "nosniff")
        self.assertEqual(headers["Cache-Control"], "no-store")
        self.assertIn("default-src 'none'", headers["Content-Security-Policy"])
        self.assertIn("script-src 'self'", headers["Content-Security-Policy"])
        self.assertNotIn(b"<script>alert", body)
        self.assertIn(b"data, never an instruction or an approval", body, "the permanent warning is in the page")
        self.assertIn(b'aria-label="Sections"', body)
        self.assertNotIn(b"http://", body.replace(b"http://127.0.0.1", b""), "no external assets")
        self.assertNotIn(b"https://", body)
        status, headers, js = self.request("/app.js")
        self.assertEqual(status, 200)
        self.assertNotIn(b"innerHTML", js, "the page only ever inserts text")
        self.assertNotIn(b"eval(", js)
        self.assertNotIn(b"https://", js, "no external requests")
        status, headers, css = self.request("/app.css")
        self.assertIn("text/css", headers["Content-Type"])
        self.assertIn(b":focus-visible", css, "keyboard focus is visible")

    def test_payload_stays_text_in_the_api(self):
        status, headers, body = self.request("/api/v1/posts/" + self.fx.ids["payload"])
        self.assertEqual(status, 200)
        self.assertIn("application/json", headers["Content-Type"])
        data = json.loads(body)
        self.assertEqual(data["summary"], PAYLOAD)
        self.assertIn("Read: this is a finding", data["body"])
        status, _, body = self.request("/api/v1/threads/" + urllib.request.quote("governance/payload", safe=""))
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(body)["posts"][0]["provenance_missing"], ["branch"])

    def test_every_read_route_answers(self):
        for route in ("health", "project", "overview", "threads", "suggestions", "amendments", "evolution", "refresh"):
            status, _, body = self.request("/api/v1/" + route)
            self.assertEqual(status, 200, route)
            json.loads(body)
        status, _, body = self.request("/api/v1/threads/" + urllib.request.quote("evolution/speedup", safe=""))
        self.assertEqual(status, 200)
        self.assertEqual(len(json.loads(body)["posts"]), 6)
        status, _, _ = self.request("/api/v1/posts/does-not-exist")
        self.assertEqual(status, 404)
        status, _, _ = self.request("/api/v1/nothing")
        self.assertEqual(status, 404)
        status, _, _ = self.request("/docs/evolution/constitution.md")
        self.assertEqual(status, 404, "no file serving")
        status, _, _ = self.request("/..%2F..%2Fetc%2Fpasswd")
        self.assertEqual(status, 404)
        status, _, body = self.request("/", method="HEAD")
        self.assertEqual((status, body), (200, b""))

    def test_writes_are_refused(self):
        for method in ("POST", "PUT", "DELETE", "PATCH"):
            status, _, body = self.request("/api/v1/overview", method=method)
            self.assertEqual(status, 405, method)
            self.assertIn("read-only", json.loads(body)["error"])
        self.assertEqual(self.fx.git_status(), "", "serving never writes to the repository")


def commons_post(commons, kind, area, thread, agent, date, summary, **kw):
    pid = post_id(date, agent)
    path = Path(commons) / "docs" / "board" / area / thread / (pid + ".md")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(post_text(kind, area, thread, agent, date, summary, **kw), encoding="utf-8")
    return pid


def tree(root):
    return sorted(str(p.relative_to(root)) for p in Path(root).rglob("*") if p.is_file() and ".git" not in p.parts)


class HubTestCase(unittest.TestCase):
    """ADR-016: names, the project registry, the commons, and the organization hub."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-hub-")).resolve()
        cls.home = cls.tmp / "pulse-home"
        cls.state_root = cls.tmp / "cache"
        cls.alpha = Fixture(cls.tmp, name="alpha-app")
        cls.beta = Fixture(cls.tmp, name="beta-service")
        # Two named agents in alpha; the rest of the fixture stays anonymous.
        cls.alpha.post("finding", "governance", "names", "copilot-cli/gpt-x", "2026-09-09T10:00:00Z",
                       "Named finding", session="s-sol", name="Sol")
        cls.alpha.post("claim", "governance", "names", "claude-code/opus", "2026-09-09T11:00:00Z",
                       "Working on names", session="s-luna", name="Luna",
                       expires="2026-09-12T00:00:00Z", files="[docs/x.md]")
        cls.alpha._git_init()
        cls.env_before = {k: os.environ.get(k) for k in ("PULSE_HOME", "PULSE_COMMONS", "PULSE_DASHBOARD_HOME")}
        os.environ["PULSE_HOME"] = str(cls.home)
        os.environ["PULSE_DASHBOARD_HOME"] = str(cls.state_root)
        os.environ.pop("PULSE_COMMONS", None)
        for fx in (cls.alpha, cls.beta):
            dash.register_project(cls.home, fx.repo)
        cls.commons, cls.created = dash.init_commons(cls.home)
        q = commons_post(cls.commons, "question", "questions", "sed-on-macos", "copilot-cli/gpt-x",
                         "2026-09-09T12:00:00Z", "Does BSD sed -i need a suffix?", session="s-sol",
                         name="Sol", project="alpha-app")
        commons_post(cls.commons, "answer", "questions", "sed-on-macos", "claude-code/opus",
                     "2026-09-09T12:30:00Z", "Yes: sed -i '' on macOS", session="s-luna",
                     name="Luna", project="beta-service", on=q)
        commons_post(cls.commons, "finding", "pitfalls", "private-var", "runner-b/model-b",
                     "2026-09-09T13:00:00Z", "/tmp resolves to /private/tmp on macOS", session="s-b",
                     project="beta-service")
        cls.commons_tree = tree(cls.commons)
        cls.hub = dash.HubState(cls.home, cls.state_root, refresh_seconds=3600)
        cls.server = dash.DashboardServer(None, port=0, hub=cls.hub)
        cls.thread = threading.Thread(target=cls.server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True)
        cls.thread.start()
        cls.url = cls.server.url.rstrip("/")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        for k, v in cls.env_before.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def get(self, path, method="GET"):
        req = urllib.request.Request(self.url + path, method=method)
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                return resp.status, dict(resp.headers), resp.read()
        except urllib.error.HTTPError as exc:
            with exc:
                return exc.code, dict(exc.headers), exc.read()

    def api(self, path, expect=200):
        status, _, body = self.get(path)
        self.assertEqual(status, expect, (path, body[:200]))
        return json.loads(body)

    def run_main(self, argv, expect=0):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            rc = dash.main(argv)
        self.assertEqual(rc, expect, err.getvalue())
        return out.getvalue(), err.getvalue()

    def test_names_are_display_only_and_fall_back_to_the_agent_id(self):
        o = self.api("/p/alpha-app/api/v1/overview")
        by_id = {c["id"]: c for c in o["active_claims"]}
        luna = by_id[post_id("2026-09-09T11:00:00Z", "claude-code/opus")]
        self.assertEqual((luna["name"], luna["agent"], luna["session"]), ("Luna", "claude-code/opus", "s-luna"))
        anonymous = by_id[self.alpha.ids["claim_active"]]
        self.assertEqual(anonymous["name"], "", "an unnamed agent keeps an empty name; the page shows the id")
        thread = self.api("/p/alpha-app/api/v1/threads/" + urllib.request.quote("governance/names", safe=""))
        self.assertEqual([(p["kind"], p["name"]) for p in thread["posts"]], [("finding", "Sol"), ("claim", "Luna")])
        self.assertTrue(all(p["project"] == "" for p in thread["posts"]), "project: is a commons field")
        # The tally is untouched by names: suggestions still count agent ids.
        sugg = self.api("/p/alpha-app/api/v1/suggestions")
        eligible = next(s for s in sugg if s["id"] == self.alpha.ids["eligible"])
        self.assertEqual((eligible["voters"], eligible["score"], eligible["status"]), (3, 1, "eligible"))
        self.assertIn("name", eligible["votes"][0])
        self.assertEqual(self.alpha.board_text("--queue"), self.beta.board_text("--queue").replace("beta-service", "alpha-app"))

    def test_registry_slugs_collisions_and_guard(self):
        registry = dash.load_registry(self.home)
        self.assertEqual([e["slug"] for e in registry["projects"]], ["alpha-app", "beta-service"])
        self.assertEqual(stat.S_IMODE((self.home / "projects.json").stat().st_mode) & 0o077, 0, "owner-only registry")
        # Same directory name elsewhere -> a numbered slug; re-registering refreshes, never duplicates.
        other = self.tmp / "elsewhere" / "alpha-app"
        (other / "docs").mkdir(parents=True)
        entry = dash.register_project(self.home, other)
        self.assertEqual(entry["slug"], "alpha-app-2")
        again = dash.register_project(self.home, other, remote="git@example.invalid:x.git")
        self.assertEqual(again["slug"], "alpha-app-2")
        self.assertEqual(len(dash.load_registry(self.home)["projects"]), 3)
        self.assertEqual(dash.find_project(dash.load_registry(self.home), slug="alpha-app-2")["remote"],
                         "git@example.invalid:x.git")
        self.assertEqual(dash.unregister_project(self.home, slug="alpha-app-2"), 1)
        self.assertIsNone(dash.unregister_project(self.home, slug="alpha-app-2"))
        self.assertEqual(len(dash.load_registry(self.home)["projects"]), 2)
        # A directory literally named "commons" never takes the commons' URL.
        c = self.tmp / "elsewhere" / "commons"
        (c / "docs").mkdir(parents=True)
        self.assertEqual(dash.register_project(self.home, c)["slug"], "commons-project")
        dash.unregister_project(self.home, repo=c)
        # The registry must live outside every repository.
        with self.assertRaises(dash.DashboardError):
            dash.register_project(self.alpha.repo / ".pulse", self.alpha.repo)
        self.assertEqual(self.alpha.git_status(), "")

    def test_commons_init_is_idempotent_and_never_lands_inside_a_repository(self):
        self.assertEqual(self.commons, self.home / "commons")
        self.assertIn("README.md", self.created)
        self.assertIn("docs/board/README.md", self.created)
        self.assertIn("docs/board/_template-post.md", self.created)
        self.assertIn(".git", self.created)
        self.assertTrue(dash.commons_present(self.commons))
        readme = (self.commons / "README.md").read_text(encoding="utf-8")
        self.assertIn("Secrets, credentials", readme, "the commons README says what never belongs there")
        target, created = dash.init_commons(self.home)
        self.assertEqual((target, created), (self.commons, []), "a second init creates nothing")
        with self.assertRaises(dash.DashboardError):
            dash.init_commons(self.home, path=self.alpha.repo / "commons")
        self.assertEqual(self.alpha.git_status(), "")
        # Location precedence: PULSE_COMMONS, then the registry, then <home>/commons.
        os.environ["PULSE_COMMONS"] = str(self.tmp / "env-commons")
        try:
            self.assertEqual(dash.commons_dir(self.home), (self.tmp / "env-commons").resolve())
        finally:
            os.environ.pop("PULSE_COMMONS")
        self.assertEqual(dash.commons_dir(self.home, {"projects": [], "commons": str(self.tmp / "reg")}),
                         (self.tmp / "reg").resolve())
        # The wrapper resolves the same place and hands the read to board.sh.
        env = dict(os.environ, PULSE_HOME=str(self.home))
        env.pop("PULSE_COMMONS", None)
        out = subprocess.run(["bash", str(SCRIPTS / "commons.sh"), "--path"], env=env,
                             capture_output=True, text=True, check=True).stdout.strip()
        self.assertEqual(Path(out).resolve(), self.commons)
        out = subprocess.run(["bash", str(SCRIPTS / "commons.sh"), "--area", "questions", "--now", NOW],
                             env=env, capture_output=True, text=True, check=True).stdout
        self.assertIn("questions/sed-on-macos", out)
        self.assertNotIn("pitfalls/private-var", out, "--area bounds the read")
        self.assertIn("nothing to amend", out, "the commons has no constitution")
        missing = subprocess.run(["bash", str(SCRIPTS / "commons.sh"), "--threads"],
                                 env=dict(env, PULSE_HOME=str(self.tmp / "nowhere")), capture_output=True, text=True)
        self.assertEqual(missing.returncode, 2)
        self.assertIn("commons.sh --init", missing.stderr)

    def test_hub_pages_and_organization_routes(self):
        for path, mode, base in (("/", "org", ""), ("/p/alpha-app/", "project", "/p/alpha-app"),
                                 ("/p/alpha-app", "project", "/p/alpha-app"), ("/commons/", "commons", "/commons")):
            status, headers, body = self.get(path)
            self.assertEqual(status, 200, path)
            self.assertIn(('data-mode="%s" data-base="%s" data-hub="1"' % (mode, base)).encode(), body, path)
            self.assertIn("default-src 'none'", headers["Content-Security-Policy"])
        org = self.api("/api/v1/org")
        self.assertEqual(org["totals"]["projects"], 2)
        self.assertEqual(org["totals"]["projects_present"], 2)
        self.assertEqual(org["totals"]["questions_open"], 1)
        self.assertEqual(org["totals"]["votes_open"], 6, "2 suggestions in vote + 1 open amendment per project")
        self.assertEqual({p["slug"] for p in org["projects"]}, {"alpha-app", "beta-service"})
        alpha = next(p for p in org["projects"] if p["slug"] == "alpha-app")
        self.assertEqual((alpha["base"], alpha["present"], alpha["claims_active"]), ("/p/alpha-app", True, 2))
        self.assertEqual((org["commons"]["present"], org["commons"]["base"], org["commons"]["threads_open"]),
                         (True, "/commons", 2))
        self.assertEqual(sorted(c["slug"] for c in org["active_claims"]), ["alpha-app", "alpha-app", "beta-service"])
        self.assertEqual(org["totals"]["agents_active"], 2, "runner-a holds a claim in both projects: one identity")
        roster = self.api("/api/v1/roster")
        sol = next(p for p in roster if p["session"] == "s-sol")
        self.assertEqual((sol["name"], sol["questions"]), ("Sol", 1))
        self.assertEqual(sol["projects"], ["alpha-app", "commons (from alpha-app)"])
        luna = next(p for p in roster if p["name"] == "Luna")
        self.assertEqual(len(luna["active_claims"]), 1)
        self.assertEqual(roster[0]["active_claims"] != [], True, "claim holders are listed first")
        votes = self.api("/api/v1/votes")
        self.assertEqual([(s["slug"], s["status"]) for s in votes["suggestions"]],
                         [("alpha-app", "eligible"), ("beta-service", "eligible"),
                          ("alpha-app", "needs votes"), ("beta-service", "needs votes")])
        self.assertTrue(all(s["quorum"] == 2 and s["base"].startswith("/p/") for s in votes["suggestions"]))
        self.assertEqual(len(votes["amendments"]), 2)
        activity = self.api("/api/v1/activity")
        self.assertEqual(activity[0]["slug"], "commons")
        self.assertEqual((activity[0]["project"], activity[0]["kind"]), ("beta-service", "finding"))
        self.assertTrue(all(p["date"] >= q["date"] for p, q in zip(activity, activity[1:])), "newest first")
        commons_threads = self.api("/commons/api/v1/threads")
        self.assertEqual({t["area"] for t in commons_threads}, {"questions", "pitfalls"})
        self.assertEqual(self.api("/p/beta-service/api/v1/overview")["claims_active"], 1, "projects stay separate")
        beta_threads = {t["key"] for t in self.api("/p/beta-service/api/v1/threads")}
        self.assertNotIn("governance/names", beta_threads)
        health = self.api("/api/v1/health")
        self.assertEqual(health["projects"], ["alpha-app", "beta-service", "commons"])
        self.assertEqual(health["errors"], {})

    def test_hub_refuses_the_rest(self):
        status, _, body = self.get("/p/nope/")
        self.assertEqual(status, 404)
        self.assertIn(b'data-mode="org" data-base="" data-hub="1" data-missing="nope"', body,
                      "a human at an unknown slug gets the organization page with the reason")
        status, _, body = self.get("/p/nope/api/v1/overview")
        self.assertEqual((status, json.loads(body)["error"]), (404, "no such project"))
        self.assertEqual(self.get("/p/Bad_Slug/")[0], 404)
        self.assertEqual(self.get("/api/v1/overview")[0], 404, "project routes do not exist at the hub root")
        self.assertEqual(self.get("/api/v1/nothing")[0], 404)
        self.assertEqual(self.get("/p/alpha-app/docs/board/README.md")[0], 404, "no file serving")
        self.assertEqual(self.get("/p/alpha-app/api/v1/../../etc")[0], 404)
        for method in ("POST", "PUT", "DELETE", "PATCH"):
            status, _, body = self.get("/api/v1/org", method=method)
            self.assertEqual(status, 405, method)
        status, _, body = self.get("/", method="HEAD")
        self.assertEqual((status, body), (200, b""))
        status, _, js = self.get("/app.js")
        self.assertEqual(status, 200)
        for bad in (b"innerHTML", b"eval(", b"https://"):
            self.assertNotIn(bad, js)

    def test_hub_follows_the_registry_and_never_writes_to_a_repository(self):
        gamma = self.tmp / "gamma-tool"
        (gamma / "docs" / "board" / "questions" / "t").mkdir(parents=True)
        commons_post(gamma, "question", "questions", "t", "runner-g/model-g", "2026-09-09T14:00:00Z",
                     "Gamma asks", session="s-g")
        dash.register_project(self.home, gamma)
        try:
            health = self.api("/api/v1/health")
            self.assertIn("gamma-tool", health["projects"], "a new registration appears without a restart")
            self.assertEqual(self.api("/p/gamma-tool/api/v1/overview")["threads_open"], 1)
            shutil.rmtree(gamma / "docs")
            self.hub.reload(force=True)
            status, _, body = self.get("/p/gamma-tool/")
            self.assertEqual(status, 503)
            self.assertIn(b'data-missing="gamma-tool"', body, "the page explains a registered-but-missing project")
            status, _, body = self.get("/p/gamma-tool/api/v1/overview")
            self.assertEqual(status, 503)
            self.assertIn("missing", json.loads(body)["error"])
            org = self.api("/api/v1/org")
            missing = next(p for p in org["projects"] if p["slug"] == "gamma-tool")
            self.assertEqual((missing["present"], missing["missing"]), (False, True))
        finally:
            dash.unregister_project(self.home, slug="gamma-tool")
            shutil.rmtree(gamma, ignore_errors=True)
        self.assertEqual(self.alpha.git_status(), "")
        self.assertEqual(self.beta.git_status(), "")
        self.assertEqual(tree(self.commons), self.commons_tree, "the hub reads the commons; it never writes there")
        self.assertFalse((self.alpha.repo / ".pulse").exists())
        for db in self.state_root.rglob("board.sqlite"):
            self.assertNotIn(str(self.tmp / "alpha-app"), str(db.parent.parent), "projections live in the state root")

    def test_cli_register_projects_unregister_and_no_register(self):
        home2 = self.tmp / "home2"
        out, _ = self.run_main(["register", "--repo", str(self.beta.repo), "--home", str(home2)])
        self.assertIn("Registered /p/beta-service/", out)
        out, _ = self.run_main(["projects", "--home", str(home2)])
        self.assertIn("/p/beta-service/", out)
        self.assertIn("not created: commons.sh --init", out)
        out, _ = self.run_main(["index", "--repo", str(self.alpha.repo), "--home", str(home2), "--no-register",
                                "--state-dir", str(self.tmp / "cache2"), "--now", NOW])
        self.assertNotIn("Registered", out)
        self.assertEqual([e["slug"] for e in dash.load_registry(home2)["projects"]], ["beta-service"])
        out, _ = self.run_main(["index", "--repo", str(self.alpha.repo), "--home", str(home2),
                                "--state-dir", str(self.tmp / "cache2"), "--now", NOW])
        self.assertIn("Registered: /p/alpha-app/", out)
        out, _ = self.run_main(["unregister", "--slug", "alpha-app", "--home", str(home2)])
        self.assertIn("Removed 1 entry", out)
        self.run_main(["unregister", "--slug", "alpha-app", "--home", str(home2)], expect=1)
        self.run_main(["unregister", "--home", str(home2)], expect=1)
        out, _ = self.run_main(["commons-init", "--home", str(home2), "--path", str(self.tmp / "custom-commons")])
        self.assertIn("Commons: %s" % (self.tmp / "custom-commons"), out)
        self.assertEqual(dash.load_registry(home2)["commons"], str(self.tmp / "custom-commons"))
        self.assertEqual(dash.commons_dir(home2), self.tmp / "custom-commons")
        self.assertTrue((self.tmp / "custom-commons" / ".git").is_dir())
        self.assertEqual(self.alpha.git_status(), "")
        self.assertEqual(self.beta.git_status(), "")


if __name__ == "__main__":
    unittest.main()
