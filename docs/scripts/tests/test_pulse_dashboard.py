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

import gzip
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from contextlib import redirect_stderr, redirect_stdout
from datetime import datetime, timezone
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

    def test_vote_reason_uses_post_summary(self):
        """The projection uses the vote post's frontmatter summary as the displayed reason."""
        suggestions = self.query(dash.q_suggestions)
        by_id = {s["id"]: s for s in suggestions}
        s1 = by_id[self.fx.ids["eligible"]]
        votes = {v["agent"]: v for v in s1["votes"]}
        # runner-b/model-b voted with summary "up: fewer retries" in the fixture
        self.assertEqual(votes["runner-b/model-b"]["reason"], "up: fewer retries")

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

    def test_proposals_for_the_human_match_board_sh(self):
        """WU-008: an answer with resolution: agreed is a proposal awaiting the human;
        a later result with accepted or declined records the answer. Same tally as
        `board.sh --proposals`; the dashboard only shows it."""
        tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-proposals-")).resolve()
        try:
            fx = Fixture(tmp, name="proposal-repo")
            q = fx.post("question", "governance", "one-proposal", "runner-a/model-a", "2026-09-08T08:00:00Z",
                        "Panel: settle D1-D8", session="s-a")
            fx.post("opinion", "governance", "one-proposal", "runner-b/model-b", "2026-09-08T08:30:00Z",
                    "D1 score", session="s-b", on=q)
            agreed = fx.post("answer", "governance", "one-proposal", "runner-a/model-a", "2026-09-08T09:00:00Z",
                             "We agreed on one plan. Yes or no?", session="s-a", on=q, resolution="agreed",
                             body="D1: score. D2: yes.\n\n**Proposal for the human:** Grade the warning as a score\n"
                                  "and adopt the step-9 sentence.\nYes or no?\n\nDissent: none.\n")
            older = fx.post("answer", "governance", "older", "runner-b/model-b", "2026-09-01T09:00:00Z",
                            "Rename the queue", session="s-b", resolution="agreed",
                            body="Rename the experiment queue to the backlog. Yes or no?\n")
            said_no = fx.post("result", "governance", "older", "runner-b/model-b", "2026-09-02T09:00:00Z",
                              "The human said no: keep the name", session="s-b", on=older, resolution="declined",
                              body="Human, in chat: no.\n")
            fx._git_init()
            state_dir = dash.project_state_dir(fx.repo, tmp / "home")
            db = dash.build_projection(fx.repo, state_dir, now=NOW)
            rows = query_db(db, dash.q_proposals)
            self.assertEqual([(r["thread_key"], r["id"], r["status"]) for r in rows],
                             [("governance/one-proposal", agreed, "awaiting"), ("governance/older", older, "declined")])
            self.assertEqual(rows[0]["proposal_text"],
                             "Grade the warning as a score and adopt the step-9 sentence. Yes or no?")
            self.assertEqual((rows[0]["decided_id"], rows[0]["decided_summary"]), ("", ""))
            self.assertEqual((rows[1]["decided_id"], rows[1]["decided_date"], rows[1]["decided_summary"]),
                             (said_no, "2026-09-02T09:00:00Z", "The human said no: keep the name"))
            self.assertEqual(rows[1]["proposal_text"], "Rename the experiment queue to the backlog. Yes or no?",
                             "without a marked paragraph the first paragraph is the proposal")
            self.assertEqual([r["id"] for r in query_db(db, dash.q_proposals, True)], [agreed])
            o = query_db(db, dash.q_overview)
            self.assertEqual((o["proposals_awaiting"], o["proposals_decided"]), (1, 1))
            self.assertEqual(o["threads_closed"], 3, "agreed closes the agents' debate; the human still decides")
            text = fx.board_text("--proposals")
            self.assertIn("awaiting   governance/one-proposal", text)
            self.assertIn("read docs/board/governance/one-proposal/%s.md" % agreed, text)
            self.assertIn("declined   governance/older", text)
            self.assertIn("for the human: 1 proposal awaiting a yes or no - board.sh --proposals",
                          fx.board_text("--area", "governance"))
            records = fx.board_text("--records")
            props = [l.split("\t") for l in records.splitlines() if l.startswith("proposal\t")]
            self.assertEqual({(p[1], p[5]) for p in props},
                             {("governance/one-proposal", "awaiting"), ("governance/older", "declined")})
            self.assertEqual(fx.git_status(), "", "the tally never writes")
        finally:
            shutil.rmtree(tmp, ignore_errors=True)

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

    def test_timeline_buckets_are_continuous_and_lose_nothing(self):
        rows = [("2026-09-10T11:30:00Z", "vote"), ("2026-09-10T11:05:00Z", "vote"),
                ("2026-09-09T13:00:00Z", "claim"),       # first hour of the 24-hour window
                ("2026-09-09T12:59:59Z", "claim"),       # a day-bucket post just outside the hours
                ("2026-08-12", "finding"),               # bare date on the first day of the window
                ("2026-08-11T23:59:59Z", "finding"),     # before the window
                ("2026-09-11T00:00:00Z", "result"),      # after now (clock skew): clamped into the last bucket
                ("not a date", "opinion"), ("", None)]
        tl = dash.timeline_buckets(rows, NOW)
        self.assertEqual(tl["now"], NOW)
        self.assertEqual((len(tl["hours"]), len(tl["days"])), (24, 30))
        self.assertEqual((tl["hours"][0]["t"], tl["hours"][-1]["t"]), ("2026-09-09T13", "2026-09-10T12"))
        self.assertEqual((tl["days"][0]["t"], tl["days"][-1]["t"]), ("2026-08-12", "2026-09-10"))
        self.assertEqual((tl["total"], tl["before_window"], tl["unparsed"]), (9, 1, 2))
        self.assertEqual(sum(b["n"] for b in tl["days"]), 6)
        self.assertEqual(sum(b["n"] for b in tl["hours"]), 4)
        by_hour = {b["t"]: b for b in tl["hours"]}
        self.assertEqual(by_hour["2026-09-10T11"], {"t": "2026-09-10T11", "n": 2, "kinds": {"vote": 2}})
        self.assertEqual(by_hour["2026-09-10T12"]["kinds"], {"result": 1})
        self.assertEqual(by_hour["2026-09-09T13"]["kinds"], {"claim": 1})
        by_day = {b["t"]: b for b in tl["days"]}
        self.assertEqual(by_day["2026-08-12"]["kinds"], {"finding": 1})
        self.assertEqual(by_day["2026-09-09"]["n"], 2)
        self.assertEqual(dash.timeline_buckets([], "garbage")["days"][-1]["t"],
                         datetime.now(timezone.utc).strftime("%Y-%m-%d"), "an unreadable reference time falls back to now")
        via_db = self.query(dash.q_timeline)
        self.assertEqual(via_db["now"], NOW, "the projection's frozen reference time drives the axis")
        overview = self.query(dash.q_overview)
        self.assertEqual(sum(b["n"] for b in via_db["days"]) + via_db["before_window"] + via_db["unparsed"],
                         overview["posts"])
        self.assertEqual(via_db["before_window"], 1, "the 2026-08-01 suggestion is older than the 30-day window")

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


class AssetTestCase(unittest.TestCase):
    """The shipped page is three strings; a rule nothing renders is weight the page carries for nothing."""

    def test_page_declares_an_icon_so_browsers_do_not_ask_for_favicon_ico(self):
        # Without a declared icon every browser fetches /favicon.ico, the hub answers 404, and the
        # console logs an error on each first visit; an empty data: icon ends the request.
        self.assertIn('<link rel="icon" href="data:,">', dash.INDEX_HTML)

    def test_every_css_class_is_referenced_by_the_page_or_the_script(self):
        css, js, html = dash.APP_CSS, dash.APP_JS, dash.INDEX_HTML
        text = js + html
        # classes built at runtime from a literal prefix, e.g. 'k-' + kind
        prefixes = set(re.findall(r"""['"]([a-zA-Z][\w-]*-)['"]\s*\+""", js))
        classes = sorted(set(re.findall(r"\.([a-zA-Z_][\w-]*)", css)))
        dead = [c for c in classes if c not in text and not any(c.startswith(p) for p in prefixes)]
        self.assertEqual(dead, [], "CSS classes no view uses: %s" % ", ".join(dead))

    def test_every_design_token_is_declared_once(self):
        css = dash.APP_CSS
        # Colors carry both themes in one light-dark() value, so a token written twice is the light set
        # copied into a second rule; --shadow is a shadow list, not a color, and may follow the scheme.
        names = re.findall(r"(--[\w-]+)\s*:", css)
        repeated = sorted(n for n in set(names) if names.count(n) > 1 and n != "--shadow")
        self.assertEqual(repeated, [], "design tokens declared more than once: %s" % ", ".join(repeated))
        self.assertIn("light-dark(", css, "themes share one token block")


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

    def request(self, path, method="GET", data=None, headers=None):
        req = urllib.request.Request(self.url + path, data=data, headers=headers or {}, method=method)
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
        self.assertIn(b'id="theme"', body, "the theme toggle is part of the page")
        status, headers, js = self.request("/app.js")
        self.assertEqual(status, 200)
        self.assertNotIn(b"innerHTML", js, "the page only ever inserts text")
        self.assertNotIn(b"eval(", js)
        self.assertNotIn(b"https://", js, "no external requests")
        self.assertNotIn(b"http://", js.replace(b"http://www.w3.org/2000/svg", b""),
                         "the SVG namespace identifier is the only http literal; it is never fetched")
        self.assertIn(b"createElementNS", js, "the chart is drawn with DOM calls, not markup")
        status, headers, css = self.request("/app.css")
        self.assertIn("text/css", headers["Content-Type"])
        self.assertIn(b":focus-visible", css, "keyboard focus is visible")
        self.assertIn(b"prefers-color-scheme", css, "light and dark follow the system unless toggled")
        self.assertIn(b"prefers-reduced-motion", css)

    def test_wu005_static_markdown_and_motion_assets(self):
        status, _, js = self.request("/app.js")
        self.assertEqual(status, 200)
        self.assertIn(b"function renderMarkdown", js)
        for bad in (b"innerHTML", b"insertAdjacentHTML", b"eval(", b"https://"):
            self.assertNotIn(bad, js)
        self.assertNotIn(b"http://", js.replace(b"http://www.w3.org/2000/svg", b""))
        self.assertNotIn(b"href=\"http", js)
        status, _, css = self.request("/app.css")
        self.assertEqual(status, 200)
        self.assertIn(b"prefers-reduced-motion: no-preference", css)

    def test_timeline_route_loses_no_post(self):
        status, _, body = self.request("/api/v1/timeline")
        self.assertEqual(status, 200)
        tl = json.loads(body)
        posts = json.loads(self.request("/api/v1/overview")[2])["posts"]
        self.assertEqual((len(tl["hours"]), len(tl["days"])), (dash.TIMELINE_HOURS, dash.TIMELINE_DAYS))
        self.assertEqual(tl["total"], posts)
        self.assertEqual(sum(b["n"] for b in tl["days"]) + tl["before_window"] + tl["unparsed"], posts,
                         "every post is in a day bucket, before the window, or reported as unparsed")
        self.assertTrue(all(sum(b["kinds"].values()) == b["n"] for b in tl["days"] + tl["hours"]))
        self.assertLessEqual(sum(b["n"] for b in tl["hours"]), sum(b["n"] for b in tl["days"][-2:]))
        self.assertEqual(tl["days"][-1]["t"], tl["now"][:10], "the axis ends at the projection's reference time")
        self.assertEqual(tl["hours"][-1]["t"], tl["now"][:13])

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
        for route in ("health", "project", "overview", "threads", "suggestions", "amendments", "evolution", "timeline",
                      "notices", "proposals", "refresh"):
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

    def test_decide_route_without_store_says_so(self):
        body = json.dumps({"post_id": "nope", "answer": "yes", "words": "ok"}).encode()
        status, _, payload = self.request("/api/v1/decide", method="POST", data=body,
                                          headers={"Content-Type": "application/json", "X-Pulse-Decide": "yes"})
        self.assertIn(status, (404, 409))
        self.assertIn("store", json.loads(payload)["error"])


class StoreModeServerTestCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-store-server-")).resolve()
        cls.home = cls.tmp / "pulse-home"
        cls.state_root = cls.tmp / "cache"
        cls.fx = Fixture(cls.tmp, name="store-app")
        cls.env_before = {k: os.environ.get(k) for k in ("PULSE_HOME", "PULSE_AGENT", "PULSE_DASHBOARD_HOME")}
        os.environ["PULSE_HOME"] = str(cls.home)
        os.environ["PULSE_DASHBOARD_HOME"] = str(cls.state_root)
        os.environ.pop("PULSE_AGENT", None)
        cls.con = dash.pulse.connect(cls.home, create=True)
        dash.pulse.add_project(cls.con, cls.fx.repo, slug="store-app", actor="test")
        dash.pulse.join_agent(cls.con, "aurora", "copilot-cli", "model-a", "s-1", display="Aurora")
        dash.pulse.join_agent(cls.con, "aurora", "copilot-cli", "model-a", "s-2", display="Aurora")
        dash.pulse.join_agent(cls.con, "boreal", "copilot-cli", "model-b", "s-old", display="Boreal")
        cls.con.execute("UPDATE agents SET jobs_done = 1, last_useful_at = ?, last_active_at = ? WHERE name = 'aurora'",
                        ("2026-09-10T10:00:00Z", "2026-09-10T11:00:00Z"))
        cls.proposal = dash.pulse.create_post(
            cls.con, "answer", "store-app", "governance", "one-proposal",
            "We agreed: ship slice 2?", "Proposal for the human: ship slice 2 now.",
            agent="aurora", resolution="agreed")
        cls.finding = dash.pulse.create_post(
            cls.con, "finding", "store-app", "governance", "roster",
            "Roster should use agent names", "one row per agent", agent="boreal")
        cls.con.execute("UPDATE agents SET status = 'pruned', pruned_at = ?, pruned_reason = ?, last_active_at = ? "
                        "WHERE name = 'boreal'",
                        ("2026-09-01T00:00:00Z", "no useful output for 14 days", "2026-08-20T00:00:00Z"))
        cls.con.execute("UPDATE agents SET posts = 1 WHERE name = 'boreal'")
        cls.hub = dash.HubState(cls.home, cls.state_root, refresh_seconds=3600)
        cls.server = dash.DashboardServer(None, port=0, hub=cls.hub)
        cls.thread = threading.Thread(target=cls.server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True)
        cls.thread.start()
        cls.url = cls.server.url.rstrip("/")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        cls.con.close()
        for k, v in cls.env_before.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def request(self, path, method="GET", data=None, headers=None):
        req = urllib.request.Request(self.url + path, data=data, headers=headers or {}, method=method)
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                return resp.status, dict(resp.headers), resp.read()
        except urllib.error.HTTPError as exc:
            with exc:
                return exc.code, dict(exc.headers), exc.read()

    def api(self, path, expect=200):
        status, _, body = self.request(path)
        self.assertEqual(status, expect, (path, body[:200]))
        return json.loads(body)

    def decide(self, post_id=None, answer="yes", words="ship it", headers=None):
        body = json.dumps({"post_id": post_id or self.proposal["id"], "answer": answer, "words": words}).encode()
        h = {"Content-Type": "application/json"}
        if headers is None:
            h["X-Pulse-Decide"] = "yes"
        else:
            h.update(headers)
        return self.request("/api/v1/decide", method="POST", data=body, headers=h)

    def test_store_roster_groups_by_agent_name_and_carries_scorecard(self):
        rows = self.api("/api/v1/roster")
        by_agent = {r["agent"]: r for r in rows}
        self.assertIn("aurora", by_agent)
        self.assertIn("boreal", by_agent)
        self.assertEqual(len([r for r in rows if r["agent"] == "aurora"]), 1)
        self.assertEqual(by_agent["aurora"]["name"], "Aurora")
        self.assertEqual(by_agent["aurora"]["kind"], "session")
        self.assertEqual(by_agent["aurora"]["status"], "active")
        self.assertEqual(by_agent["aurora"]["jobs_done"], 1)
        self.assertEqual(by_agent["aurora"]["session_count"], 2)
        self.assertEqual(by_agent["aurora"]["sessions"], ["s-1", "s-2"])
        self.assertEqual(by_agent["boreal"]["status"], "pruned")
        self.assertEqual(by_agent["boreal"]["pruned_reason"], "no useful output for 14 days")

    def test_executors_route_lists_this_machine_and_every_pair_of_hands(self):
        con = dash.pulse.connect(self.home)
        try:
            dash.pulse.add_executor(con, "vm1", "pulse@10.0.0.4", "/srv/pulse", workers=2, projects="store-app", now="2026-09-10T12:00:00Z")
        finally:
            con.close()
        rows = self.api("/api/v1/executors")
        by_name = {r["name"]: r for r in rows}
        self.assertEqual(sorted(by_name), ["local", "vm1"])
        self.assertEqual(by_name["local"]["status"], "active")
        self.assertEqual(by_name["local"]["running"], 0)
        self.assertEqual(by_name["vm1"]["status"], "drained", "added hands wait for a probe")
        self.assertEqual((by_name["vm1"]["address"], by_name["vm1"]["workers"], by_name["vm1"]["projects"]), ("pulse@10.0.0.4", 2, "store-app"))
        self.assertNotIn("Set-Cookie", self.request("/api/v1/executors")[1])

    def test_runner_route_counts_hands_in_standby_the_worker_pool_and_the_daemon(self):
        con = dash.pulse.connect(self.home)
        try:
            dash.pulse.add_executor(con, "vm2", "pulse@10.0.0.5", "/srv/pulse", workers=2, projects="store-app", now="2026-09-10T12:00:00Z")
            con.execute("UPDATE executors SET status = 'active' WHERE name = 'vm2'")
            idle = dash.pulse.spawn_agent(con, runner="copilot", model="model-w", kind="worker")["name"]
            busy = dash.pulse.spawn_agent(con, runner="copilot", model="model-w", kind="worker")["name"]
            con.execute("INSERT INTO jobs (id, project, kind, title, status, created_at, lease_agent) VALUES (?, 'store-app', 'improve', 'leased one', 'leased', ?, ?)",
                        ("J-leased", dash.pulse.utc_now(), busy))
            con.execute("INSERT INTO jobs (id, project, kind, title, status, created_at) VALUES (?, 'store-app', 'improve', 'waiting one', 'queued', ?)",
                        ("J-queued", dash.pulse.utc_now()))
            dash.pulse.daemon_heartbeat(con, loops=3, interval=300)
            quorum = dash.pulse.setting_int(con, "quorum")
            con.commit()
        finally:
            con.close()
        r = self.api("/api/v1/runner")
        self.assertTrue(r["available"])
        self.assertEqual(r["daemon"]["state"], "alive")
        self.assertEqual((r["daemon"]["pid"], r["daemon"]["loops"], r["daemon"]["interval"]), (os.getpid(), 3, 300))
        self.assertTrue(r["daemon"]["text"].startswith("alive pid"))
        self.assertEqual(r["runner"], {"allowed": True, "reason": ""})
        by_name = {x["name"]: x for x in r["executors"]}
        self.assertEqual((by_name["local"]["status"], by_name["local"]["free"]), ("active", 1), "this machine keeps one free slot")
        self.assertEqual((by_name["vm2"]["workers"], by_name["vm2"]["running"], by_name["vm2"]["free"], by_name["vm2"]["projects"]), (2, 0, 2, "store-app"))
        self.assertEqual(by_name.get("vm1", {"free": 0})["free"], 0, "drained hands never count as standby")
        self.assertEqual((r["hands"]["free"], r["hands"]["slots"], r["hands"]["executors_active"], r["hands"]["any_project"]), (3, 3, 2, True))
        self.assertEqual((r["workers"]["pool"], r["workers"]["idle"], r["workers"]["busy"], r["workers"]["target"]), (2, 1, 1, quorum + 1))
        self.assertEqual(r["workers"]["idle_names"], [idle])
        self.assertEqual(r["workers"]["prune_after_days"], 14, "the roster KPI names the prune rule from the store's setting")
        self.assertEqual((r["jobs"]["queued"], r["jobs"]["leased"], r["jobs"]["blocked"], r["jobs"]["running"], r["jobs"]["runs_today"]), (1, 1, 0, 0, 0))
        self.assertIsInstance(r["jobs"]["cap"], int)
        self.assertNotIn("Set-Cookie", self.request("/api/v1/runner")[1])

    def test_runner_route_without_a_store_says_so(self):
        hub = dash.HubState(self.tmp / "no-store-home", self.tmp / "no-store-cache", refresh_seconds=3600)
        self.assertEqual(dash.q_runner(hub), {"available": False})

    def test_decide_route_records_decision_result_and_refreshes_projection(self):
        before = self.api("/api/v1/proposals")
        self.assertEqual(before[0]["status"], "awaiting")
        self.assertTrue(before[0]["can_decide"])
        status, _, body = self.decide(words="owner says yes")
        self.assertEqual(status, 200, body)
        result_id = json.loads(body)["result_post"]
        row = self.con.execute("SELECT answer, words, result_post FROM decisions WHERE post_id = ?",
                               (self.proposal["id"],)).fetchone()
        self.assertEqual((row["answer"], row["words"], row["result_post"]), ("yes", "owner says yes", result_id))
        result = dash.pulse.post_row(self.con, result_id)
        self.assertEqual((result["kind"], result["resolution"], result["on_post"]), ("result", "accepted", self.proposal["id"]))
        after = self.api("/api/v1/proposals")
        decided = next(r for r in after if r["id"] == self.proposal["id"])
        self.assertEqual(decided["status"], "accepted")

    def test_decide_route_requires_guard_header(self):
        status, _, body = self.decide(post_id=self.finding["id"], headers={})
        self.assertEqual(status, 403)
        self.assertIn("X-Pulse-Decide", json.loads(body)["error"])

    def test_decide_route_rejects_bad_answer(self):
        status, _, body = self.decide(post_id=self.finding["id"], answer="maybe")
        self.assertEqual(status, 400)
        self.assertIn("yes or no", json.loads(body)["error"])

    def test_decide_route_rejects_second_decision(self):
        prop = dash.pulse.create_post(
            self.con, "answer", "store-app", "governance", "twice",
            "We agreed: decide once?", "Proposal for the human: decide once.",
            agent="aurora", resolution="agreed")
        self.hub.state("store-app").ensure_fresh(force=True)
        self.assertEqual(self.decide(post_id=prop["id"], answer="no")[0], 200)
        status, _, body = self.decide(post_id=prop["id"], answer="yes")
        self.assertEqual(status, 400)
        self.assertIn("already decided", json.loads(body)["error"])

    def test_other_post_paths_stay_read_only(self):
        status, _, body = self.request("/api/v1/org", method="POST", data=b"{}",
                                       headers={"Content-Type": "application/json", "X-Pulse-Decide": "yes"})
        self.assertEqual(status, 405)
        self.assertIn("except POST /api/v1/decide", json.loads(body)["error"])


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
        # A proposal for the human in alpha: the panel's question, then the agreed answer (awaiting).
        pq = cls.alpha.post("question", "governance", "one-proposal", "runner-q/model-q", "2026-09-08T08:00:00Z",
                            "Which C2 fix?", session="s-q")
        cls.alpha_proposal = cls.alpha.post(
            "answer", "governance", "one-proposal", "runner-a/model-a", "2026-09-08T09:00:00Z",
            "We agreed: grade C2 as a score. Yes or no?", session="s-a", on=pq, resolution="agreed",
            body="D1: score.\n\n**Proposal for the human:** Grade the warning as a score, "
                 "not a gate. Yes or no?\n\nDissent: none.\n")
        cls.alpha._git_init()
        # The same call sign on a second agent id in beta: a collision the roster flags, never resolves.
        cls.beta.post("finding", "governance", "call-signs", "runner-z/model-z", "2026-09-09T09:00:00Z",
                      "Also called Sol", session="s-z", name="Sol")
        cls.beta._git_init()
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

    def test_proposals_for_the_human_across_the_organization(self):
        """WU-008: the agents' agreed answer is shown to the human on the hub, read-only;
        the human answers in chat and an agent records it - the page has no button."""
        org = self.api("/api/v1/org")
        self.assertEqual(org["totals"]["proposals_awaiting"], 1)
        self.assertEqual([(r["slug"], r["status"], r["id"]) for r in org["proposals"]],
                         [("alpha-app", "awaiting", self.alpha_proposal)])
        rows = self.api("/api/v1/proposals")
        self.assertEqual(len(rows), 1)
        r = rows[0]
        self.assertEqual((r["slug"], r["base"], r["status"], r["thread_key"]),
                         ("alpha-app", "/p/alpha-app", "awaiting", "governance/one-proposal"))
        self.assertEqual(r["proposal_text"], "Grade the warning as a score, not a gate. Yes or no?")
        self.assertNotIn("body", r, "the hub list carries the plain proposal, not every body")
        self.assertEqual(r["agent"], "runner-a/model-a")
        mine = self.api("/p/alpha-app/api/v1/proposals")
        self.assertEqual([p["id"] for p in mine], [self.alpha_proposal])
        self.assertIn("Proposal for the human", mine[0]["body"], "the project route keeps the body for the card")
        self.assertEqual(self.api("/p/beta-service/api/v1/proposals"), [])
        self.assertEqual(self.api("/p/alpha-app/api/v1/overview")["proposals_awaiting"], 1)
        for path in ("/", "/p/alpha-app/"):
            self.assertEqual(self.get(path)[0], 200, path)
        self.assertIn(b"#/proposals", self.get("/app.js")[2], "the nav and the For-you view ship in the one script")
        self.assertEqual(self.alpha.git_status(), "", "reading proposals never writes to the repository")

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
        self.assertNotIn(b"http://", js.replace(b"http://www.w3.org/2000/svg", b""))

    def test_roster_status_and_days_since_fields(self):
        allowed = {"working", "active", "recent", "idle", "dormant"}
        roster = self.api("/api/v1/roster")
        self.assertTrue(roster)
        self.assertTrue(all(p["status"] in allowed for p in roster))
        self.assertTrue(all(isinstance(p["days_since"], float) for p in roster))
        self.assertTrue(all(p["days_since"] >= 0 for p in roster))
        self.assertTrue(any(p["status"] == "working" and p["active_claims"] for p in roster))

    def test_brain_flag_round_trips_and_org_totals_report_it(self):
        home2 = self.tmp / "brain-home"
        state2 = self.tmp / "brain-cache"
        self.run_main(["index", "--repo", str(self.alpha.repo), "--home", str(home2),
                       "--state-dir", str(state2), "--now", NOW, "--brain"])
        self.run_main(["index", "--repo", str(self.beta.repo), "--home", str(home2),
                       "--state-dir", str(state2), "--now", NOW])
        reg = dash.load_registry(home2)
        flagged = [e["slug"] for e in reg["projects"] if e.get("brain")]
        self.assertEqual(flagged, ["alpha-app"])
        hub = dash.HubState(home2, state2, refresh_seconds=3600)
        org = dash.q_org(hub)
        self.assertEqual(org["totals"]["brain"], "alpha-app")
        self.assertEqual([p["brain"] for p in org["projects"] if p["slug"] == "alpha-app"], [True])

    def test_org_project_cards_expose_brain_boolean(self):
        home2 = self.tmp / "brain-card-home"
        state2 = self.tmp / "brain-card-cache"
        self.run_main(["index", "--repo", str(self.alpha.repo), "--home", str(home2),
                       "--state-dir", str(state2), "--now", NOW, "--brain"])
        self.run_main(["index", "--repo", str(self.beta.repo), "--home", str(home2),
                       "--state-dir", str(state2), "--now", NOW])
        org = dash.q_org(dash.HubState(home2, state2, refresh_seconds=3600))
        by_slug = {p["slug"]: p["brain"] for p in org["projects"]}
        self.assertEqual(by_slug, {"alpha-app": True, "beta-service": False})

    def test_hub_timeline_roster_collisions_and_vote_expiry(self):
        tl = self.api("/api/v1/timeline")
        self.assertEqual({s["slug"] for s in tl["series"]}, {"alpha-app", "beta-service", "commons"})
        day_labels = [b["t"] for b in tl["days"]]
        hour_labels = [b["t"] for b in tl["hours"]]
        for s in tl["series"]:
            self.assertEqual([b["t"] for b in s["days"]], day_labels, "every project shares one axis")
            self.assertEqual([b["t"] for b in s["hours"]], hour_labels)
            self.assertEqual(s["now"], tl["now"])
        self.assertEqual(sum(s["total"] for s in tl["series"]), tl["total"])
        self.assertEqual(sum(b["n"] for b in tl["days"]), sum(b["n"] for s in tl["series"] for b in s["days"]))
        commons = next(s for s in tl["series"] if s["slug"] == "commons")
        self.assertEqual((commons["kind"], commons["base"], commons["total"]), ("commons", "/commons", 3))
        roster = self.api("/api/v1/roster")
        sols = sorted((p for p in roster if p["name"] == "Sol"), key=lambda p: p["agent"])
        self.assertEqual([p["agent"] for p in sols], ["copilot-cli/gpt-x", "runner-z/model-z"])
        self.assertEqual([(p["name_shared"], p["shared_with"]) for p in sols],
                         [(True, ["runner-z/model-z"]), (True, ["copilot-cli/gpt-x"])],
                         "one call sign on two agent ids is flagged on both, never merged")
        luna = next(p for p in roster if p["name"] == "Luna")
        self.assertEqual((luna["name_shared"], luna["shared_with"]), (False, []))
        self.assertTrue(all(p["name_shared"] is False for p in roster if not p["name"]),
                        "anonymous identities never collide")
        votes = self.api("/api/v1/votes")
        self.assertTrue(all(s["expiry_days"] == 14 for s in votes["suggestions"]),
                        "the board's expiry window travels with each suggestion so the page can say when it lapses")
        self.assertEqual(self.api("/api/v1/activity")[0]["slug"], "commons", "the wider activity feed keeps its order")

    def test_notices_mirror_board_sh_and_presence_needs_no_registry_write(self):
        """WU-006: the hub carries the same notices board.sh prints (votes wanted,
        brain, commons questions) and notices a project or commons appearing or
        vanishing without a registry write."""
        n = self.api("/api/v1/notices")
        self.assertEqual(self.api("/api/v1/org")["notices"], n, "one computation, exposed twice")
        self.assertEqual({b["slug"] for b in n["ballot"]}, {"alpha-app", "beta-service"})
        for b in n["ballot"]:
            self.assertEqual(set(b), {"slug", "base", "name", "brain", "suggestions", "amendments", "proposals"})
            self.assertEqual(b["base"], "/p/" + b["slug"])
        self.assertEqual(n["votes_open"], sum(b["suggestions"] + b["amendments"] for b in n["ballot"]))
        self.assertEqual(n["proposals_awaiting"], 1, "alpha's agreed answer waits for the human")
        self.assertEqual({b["slug"]: b["proposals"] for b in n["ballot"]}, {"alpha-app": 1, "beta-service": 0})
        self.assertEqual((n["brain"], n["brains"]), ("", []), "no brain flagged in this registry")
        self.assertEqual((n["commons_present"], n["commons_base"]), (True, "/commons"))
        self.assertEqual(n["commons_questions"], self.api("/api/v1/org")["commons"]["questions_open"])
        mine = self.api("/p/alpha-app/api/v1/notices")
        self.assertEqual([b["slug"] for b in mine["ballot"]], ["alpha-app"], "a project route answers for its own board")
        self.assertEqual((mine["ballot"][0]["base"], mine["commons_present"], mine["brain"]), ("", None, ""))
        self.assertEqual(mine["proposals_awaiting"], 1)
        self.assertIn(b'id="notices"', self.get("/")[2])
        # Presence is re-evaluated on every read: the registry file does not change here.
        delta = self.tmp / "delta-lib"
        (delta / "docs" / "board" / "questions" / "t").mkdir(parents=True)
        commons_post(delta, "question", "questions", "t", "runner-d/model-d", "2026-09-09T15:00:00Z",
                     "Delta asks", session="s-d")
        dash.register_project(self.home, delta)
        try:
            self.assertTrue(next(p for p in self.api("/api/v1/org")["projects"] if p["slug"] == "delta-lib")["present"])
            shutil.rmtree(delta / "docs")
            gone = next(p for p in self.api("/api/v1/org")["projects"] if p["slug"] == "delta-lib")
            self.assertEqual((gone["present"], gone["missing"]), (False, True), "vanished without a registry write")
            self.assertNotIn("delta-lib", {b["slug"] for b in self.api("/api/v1/notices")["ballot"]})
            (delta / "docs" / "board" / "questions" / "t").mkdir(parents=True)
            commons_post(delta, "question", "questions", "t", "runner-d/model-d", "2026-09-09T15:00:00Z",
                         "Delta asks again", session="s-d")
            back = next(p for p in self.api("/api/v1/org")["projects"] if p["slug"] == "delta-lib")
            self.assertTrue(back["present"], "came back without a registry write or a restart")
        finally:
            dash.unregister_project(self.home, slug="delta-lib")
            shutil.rmtree(delta, ignore_errors=True)

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


    def test_headless_browser_smoke(self):
        """Optional: render every dashboard view in headless Chromium and fail on console errors or CSP violations.

        Looks for common Chrome/Chromium executables on PATH or in PLAYWRIGHT_BROWSERS_PATH.
        Skips the test when none is found.
        """
        import re
        from pathlib import Path

        candidates = ["chromium", "google-chrome", "chrome-headless-shell", "chrome", "chromium-browser"]
        bin_path = None
        for c in candidates:
            p = shutil.which(c)
            if p:
                bin_path = p
                break
        if bin_path is None:
            pw = os.environ.get("PLAYWRIGHT_BROWSERS_PATH")
            if pw:
                pwp = Path(pw)
                if pwp.exists():
                    for p in pwp.rglob("*"):
                        name = p.name.lower()
                        if ("chrome" in name or "chromium" in name) and p.is_file() and os.access(str(p), os.X_OK):
                            bin_path = str(p)
                            break
        if bin_path is None:
            self.skipTest("No headless Chromium/Chrome binary found on PATH or PLAYWRIGHT_BROWSERS_PATH")

        routes = ["/#/", "/#/votes", "/#/agents", "/#/activity", "/#/threads", "/#/evolution"]
        # Try each route; collect stderr and fail if console errors or CSP violations are observed.
        for r in routes:
            cmd = [bin_path, "--headless=new", "--disable-gpu", "--no-sandbox",
                   "--enable-logging=stderr", "--virtual-time-budget=6000", self.url + r]
            try:
                proc = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
            except subprocess.TimeoutExpired as exc:
                self.fail(f"Headless browser timed out loading {r}: {exc}")
            stderr = (proc.stderr or "") + "\n" + (proc.stdout or "")
            # Fail on CONSOLE lines with an error/severity marker, or common CSP violation messages.
            if re.search(r"CONSOLE.*\b(error|severe|fatal)\b", stderr, re.I) or re.search(r"\bContent-Security-Policy\b|Refused to apply inline style|CSP violation|Refused to load the script", stderr, re.I):
                self.fail(f"Headless render of {r} reported console/CSP errors:\n{stderr}")


# --------------------------------------------------------------------------- #
# ADR-023: the built page (docs/scripts/dashboard/dist) at the same routes
# --------------------------------------------------------------------------- #

UI_SRC = dash.UI_SOURCE_DIR
UI_DIST = dash.UI_DIR
# An editable twin of the owner's guard (test_pulse_dashboard_guards.py) so the loop sees the same rules
# where it can measure them: our source never inserts markup and never names an outside host (ADR-023 §3).
FORBIDDEN_UI_SRC = ("dangerouslySetInnerHTML", "innerHTML", "outerHTML", "insertAdjacentHTML", "eval(", "new Function",
                    "document.write", "http://", "https://")
W3C_NS = (b"http://www.w3.org/2000/svg", b"http://www.w3.org/1999/xhtml", b"http://www.w3.org/1998/Math/MathML",
          b"http://www.w3.org/XML/1998/namespace", b"http://www.w3.org/1998/")
JS_BUDGET_GZIP = 150 * 1024
CSS_BUDGET_GZIP = 20 * 1024
EXACT_CSP = ("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; "
             "img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")


def ui_source_files():
    files = [UI_SRC / "index.html"]
    files.extend(p for p in sorted((UI_SRC / "src").rglob("*")) if p.is_file() and p.suffix in (".ts", ".tsx", ".js", ".jsx", ".css", ".html"))
    return files


@unittest.skipUnless(UI_SRC.is_dir(), "docs/scripts/dashboard is not part of this checkout")
class BuiltUITestCase(unittest.TestCase):
    """The build is what the human gets; these tests keep dist/ honest about src/ and the server honest about dist/."""

    def test_dist_is_complete_and_matches_the_source_by_hash(self):
        ui, why = dash.load_built_ui(UI_DIST)
        self.assertIsNotNone(ui, "docs/scripts/dashboard/dist is not a complete build: %s (run `npm ci && npm run build`)" % why)
        self.assertEqual(ui.build.get("schema"), dash.UI_BUILD_SCHEMA)
        self.assertEqual(ui.build.get("files"), list(dash.UI_FILES))
        self.assertEqual(ui.source_hash, dash.ui_source_hash(UI_SRC),
                         "dist/ is stale: it was built from other source; run `npm run build` in docs/scripts/dashboard and commit dist/")
        self.assertNotIn("built_at", ui.build, "BUILD.json carries no timestamp so the same source builds the same file")

    def test_python_hash_twin_follows_the_vite_recipe(self):
        # The recipe in vite.config.ts: sorted relative posix paths; path, NUL, bytes, NUL per file.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "src").mkdir()
            (root / "index.html").write_bytes(b"<html>")
            (root / "src" / "b.ts").write_bytes(b"b")
            (root / "src" / "a.ts").write_bytes(b"a")
            (root / "vite.config.ts").write_bytes(b"v")
            (root / "node_modules").mkdir()
            (root / "node_modules" / "x.js").write_bytes(b"ignored")
            expected = hashlib.sha256()
            for rel, data in (("index.html", b"<html>"), ("src/a.ts", b"a"), ("src/b.ts", b"b"), ("vite.config.ts", b"v")):
                expected.update(rel.encode() + b"\0" + data + b"\0")
            self.assertEqual(dash.ui_source_hash(root), expected.hexdigest())

    def test_bundle_stays_inside_the_budget(self):
        js = (UI_DIST / "app.js").read_bytes()
        css = (UI_DIST / "app.css").read_bytes()
        self.assertLessEqual(len(gzip.compress(js, 9)), JS_BUDGET_GZIP, "app.js over 150 KB gzip (ADR-023 §5, P6)")
        self.assertLessEqual(len(gzip.compress(css, 9)), CSS_BUDGET_GZIP, "app.css over 20 KB gzip (ADR-023 §5, P6)")

    def test_source_inserts_only_text_and_reaches_nowhere(self):
        for path in ui_source_files():
            text = path.read_text(encoding="utf-8")
            for bad in FORBIDDEN_UI_SRC:
                self.assertNotIn(bad, text, "%s: %r" % (path.relative_to(UI_SRC), bad))

    def test_built_files_carry_no_outside_reach(self):
        js = (UI_DIST / "app.js").read_bytes()
        for bad in (b"eval(", b"new Function", b"document.write", b"XMLHttpRequest", b"https://"):
            self.assertNotIn(bad, js, "the bundle never evaluates text as code and never names an outside host: %r" % bad)
        stripped = js
        for ns in W3C_NS:
            stripped = stripped.replace(ns, b"")
        self.assertNotIn(b"http://", stripped, "W3C namespace strings are the only http literals; never fetched")
        css = (UI_DIST / "app.css").read_bytes()
        for bad in (b"@import", b"url(http", b"https://"):
            self.assertNotIn(bad, css)
        html = (UI_DIST / "index.html").read_text(encoding="utf-8")
        self.assertEqual(html.count("<script"), 1, "one script tag, loading /app.js")
        self.assertIn('src="/app.js"', html)
        self.assertIn('href="/app.css"', html)
        self.assertIn('<link rel="icon" href="data:,">', html)
        self.assertIn('<meta name="color-scheme"', html)
        self.assertNotIn("https://", html)
        self.assertNotIn("http://", html)
        self.assertNotRegex(html, r"\son[a-z]+\s*=", "no inline event handlers")
        for ph in dash.UI_PLACEHOLDERS:
            self.assertIn(ph, html, "the server fills %s at request time" % ph)

    def test_select_ui_auto_falls_back_legacy_forces_dist_refuses(self):
        with tempfile.TemporaryDirectory() as tmp:
            empty = Path(tmp)
            ui, line = dash.select_ui("auto", dist_dir=empty, source_dir=UI_SRC)
            self.assertIsNone(ui)
            self.assertIn("embedded page", line)
            self.assertIn("missing", line)
            ui, line = dash.select_ui("legacy", dist_dir=UI_DIST, source_dir=UI_SRC)
            self.assertIsNone(ui)
            self.assertIn("--ui legacy", line)
            with self.assertRaises(dash.DashboardError):
                dash.select_ui("dist", dist_dir=empty, source_dir=UI_SRC)
            with self.assertRaises(dash.DashboardError):
                dash.select_ui("nope", dist_dir=UI_DIST, source_dir=UI_SRC)
            # A BUILD.json that does not parse is an incomplete build, not a crash.
            broken = empty / "dist"
            broken.mkdir()
            for name in dash.UI_FILES:
                shutil.copy(UI_DIST / name, broken / name)
            (broken / "BUILD.json").write_text("{not json", encoding="utf-8")
            ui, why = dash.load_built_ui(broken)
            self.assertIsNone(ui)
            self.assertIn("does not parse", why)
            # A stale build is served, but the start line says so.
            (broken / "BUILD.json").write_text(json.dumps({"schema": dash.UI_BUILD_SCHEMA, "source_hash": "0" * 64,
                                                            "files": list(dash.UI_FILES), "tools": {}}), encoding="utf-8")
            ui, line = dash.select_ui("auto", dist_dir=broken, source_dir=UI_SRC)
            self.assertIsNotNone(ui)
            self.assertIn("STALE", line)
        ui, line = dash.select_ui("auto", dist_dir=UI_DIST, source_dir=UI_SRC)
        self.assertIsNotNone(ui)
        self.assertNotIn("STALE", line)
        self.assertIn("vite", line)

    def test_cli_accepts_the_ui_flag_on_serve_and_hub(self):
        parser = dash.build_parser()
        for cmd in ("serve", "hub"):
            args = parser.parse_args([cmd])
            self.assertEqual(args.ui, "auto")
            for choice in dash.UI_CHOICES:
                self.assertEqual(parser.parse_args([cmd, "--ui", choice]).ui, choice)
        with self.assertRaises(SystemExit), redirect_stderr(io.StringIO()):
            parser.parse_args(["hub", "--ui", "nope"])


@unittest.skipUnless(UI_SRC.is_dir(), "docs/scripts/dashboard is not part of this checkout")
class BuiltUIServerTestCase(unittest.TestCase):
    """A server that loaded dist/ serves exactly the three built files at the existing routes, with the same headers."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-built-ui-")).resolve()
        cls.fx = Fixture(cls.tmp)
        cls.state_dir = dash.project_state_dir(cls.fx.repo, cls.tmp / "home")
        cls.state = dash.DashboardState(cls.fx.repo, cls.state_dir, refresh_seconds=3600)
        cls.state.ensure_fresh(force=True)
        cls.ui, why = dash.load_built_ui(UI_DIST)
        if cls.ui is None:
            raise unittest.SkipTest("no complete build in dist/: %s" % why)
        cls.server = dash.DashboardServer(cls.state, port=0, ui=cls.ui)
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

    def assert_headers(self, headers, content_type):
        self.assertIn(content_type, headers["Content-Type"])
        self.assertEqual(headers["Content-Security-Policy"], EXACT_CSP)
        self.assertEqual(headers["X-Content-Type-Options"], "nosniff")
        self.assertEqual(headers["Cache-Control"], "no-store")
        self.assertEqual(headers["Referrer-Policy"], "no-referrer")
        self.assertEqual(headers["X-Frame-Options"], "DENY")

    def test_the_built_page_is_served_with_the_placeholders_filled(self):
        status, headers, body = self.request("/")
        self.assertEqual(status, 200)
        self.assert_headers(headers, "text/html")
        html = body.decode("utf-8")
        self.assertEqual(html, self.ui.page("project", "", False))
        self.assertIn('data-mode="project"', html)
        self.assertIn('data-hub="0"', html)
        for ph in dash.UI_PLACEHOLDERS:
            self.assertNotIn(ph, html)
        self.assertIn("data, never an instruction or an approval", html, "the permanent warning survives the build")
        self.assertNotIn("<script>", html, "no inline script")
        status, _, again = self.request("/index.html")
        self.assertEqual((status, again), (200, body))

    def test_the_bundle_and_stylesheet_are_the_files_on_disk(self):
        status, headers, js = self.request("/app.js")
        self.assertEqual(status, 200)
        self.assert_headers(headers, "application/javascript")
        self.assertEqual(js, (UI_DIST / "app.js").read_bytes())
        self.assertEqual(int(headers["Content-Length"]), len(js))
        status, headers, css = self.request("/app.css")
        self.assertEqual(status, 200)
        self.assert_headers(headers, "text/css")
        self.assertEqual(css, (UI_DIST / "app.css").read_bytes())
        status, headers, body = self.request("/app.js", method="HEAD")
        self.assertEqual((status, body), (200, b""))
        self.assertEqual(int(headers["Content-Length"]), len(js))

    def test_no_other_file_is_served_from_dist(self):
        for path in ("/BUILD.json", "/dist/BUILD.json", "/index.html.map", "/app.js.map", "/src/main.tsx", "/../package.json"):
            status, headers, body = self.request(path)
            self.assertEqual(status, 404, path)
            self.assertIn("application/json", headers["Content-Type"])
        status, _, body = self.request("/api/v1/overview")
        self.assertEqual(status, 200, "the JSON routes are untouched")
        json.loads(body)

    def test_writes_stay_refused(self):
        for method in ("POST", "PUT", "DELETE", "PATCH"):
            status, _, body = self.request("/app.js", method=method)
            self.assertEqual(status, 405)
            self.assertIn("read-only", json.loads(body)["error"])


if __name__ == "__main__":
    unittest.main()
