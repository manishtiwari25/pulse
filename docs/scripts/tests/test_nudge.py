#!/usr/bin/env python3
"""Tests for docs/scripts/nudge.py - the read-only cross-project work digest (ADR-018, slice 1).

Two temporary repositories with one of every signal, a registry pointing at
them, a commons with an open question, and a hash of every byte before and
after each run: the digest must rank correctly and write nothing anywhere.
Run with `python3 -m unittest docs.scripts.tests.test_nudge`.
"""
import hashlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
NOW = "2026-09-10T12:00:00Z"


def load_script(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


nudge = load_script("pulse_nudge", "nudge.py")
pulse = load_script("pulse_for_nudge_test", "pulse.py")

CONSTITUTION_ON = """---
loop: on
limits: none
---

# Constitution (nudge fixture)

## Hard stops

Never cross a hard stop.

## Immutable paths

- `docs/evolution/constitution.md`

## Editable scope by stage

| File class | Paths | Stage | Autonomy | Clean cycles required | Clean cycles so far | Incidents |
| --- | --- | --- | --- | --- | --- | --- |
| product code | `src/` | 4 | human | 5 | 0 | 0 |

## Kill switch

A file named docs/evolution/STOP halts every cycle.
"""

EVALUATORS = """---
baseline: B-001
---

# Evaluators (nudge fixture)

## Constraints (pass/fail)

- C1 the test suite passes

## Product evaluators

- P1 unit tests pass

Held out: %s
"""

ARCHIVE = """# Archive (nudge fixture)

## Baselines

| Row | Date | Tree | Constraints | S1 | Notes |
| --- | --- | --- | --- | --- | --- |
| B-001 | 2026-09-01 | v1 (abc1234) | C1 pass | 0.5 | first baseline |

## Variants

| Row | Date | Parent | Suggestion | Votes | File class | Change (one variable) | Hypothesis | Target: before -> after | Other scores | Decision | Ref |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

## Incidents

| Row | Date | File class | What happened | Variant | Action |
| --- | --- | --- | --- | --- | --- |
"""


def post_text(kind, area, thread, agent, date, summary, session="s-1", **extra):
    runner, _, model = agent.partition("/")
    lines = ["---", "kind: %s" % kind, "area: %s" % area, "thread: %s" % thread, "agent: %s" % agent,
             "runner: %s" % runner, "model: %s" % model, "session: %s" % session, "date: %s" % date,
             "branch: main", "links: []", "summary: %s" % summary]
    for key, value in extra.items():
        lines.append("%s: %s" % (key, value))
    lines += ["---", "", "## %s" % kind, "", summary, ""]
    return "\n".join(lines)


def post_id(date, agent):
    return date.replace("-", "").replace(":", "") + "-" + agent.replace("/", "_")


def sha256_tree(root):
    digest = hashlib.sha256()
    root = Path(root)
    for path in sorted(p for p in root.rglob("*") if p.is_file() and ".git" not in p.parts):
        digest.update(str(path.relative_to(root)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


def git_init(repo):
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")
    subprocess.run(["git", "init", "-q", str(repo)], env=env, check=True, capture_output=True)
    for cmd in (["git", "checkout", "-q", "-B", "main"], ["git", "add", "-A"],
                ["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture"]):
        subprocess.run(cmd, cwd=str(repo), env=env, check=True, capture_output=True)


class Org:
    """Two projects, a registry, and a commons, all under one temporary root."""

    A, B, C, D = "runner-a/model-a", "runner-b/model-b", "runner-c/model-c", "runner-d/model-d"

    def __init__(self, root):
        self.root = Path(root)
        self.home = self.root / "home"
        self.home.mkdir()
        self.ids = {}
        self.open_loop = self._project("open-loop", held_out="H01, H02", stop=False)
        self.gated = self._project("gated", held_out="pending", stop=False)
        self.stopped = self._project("stopped", held_out="H01", stop=True)
        self.commons = self.root / "commons"
        (self.commons / "docs" / "board").mkdir(parents=True)
        (self.commons / "docs" / "board" / "README.md").write_text("# Commons (fixture)\n")
        self.ids["commons_question"] = self._post(self.commons, "question", "questions", "what-do-you-want", self.A,
                                                  "2026-09-04T15:12:00Z", "To every project - what do you want?")
        git_init(self.commons)
        registry = {"version": 1, "commons": str(self.commons), "projects": [
            {"slug": "open-loop", "path": str(self.open_loop), "remote": "", "registered": "2026-09-01T00:00:00Z", "brain": True},
            {"slug": "gated", "path": str(self.gated), "remote": "", "registered": "2026-09-01T00:00:00Z"},
            {"slug": "stopped", "path": str(self.stopped), "remote": "", "registered": "2026-09-01T00:00:00Z"},
            {"slug": "ghost", "path": str(self.root / "nowhere"), "remote": "", "registered": "2026-09-01T00:00:00Z"},
        ]}
        self.registry = self.home / "projects.json"
        self.registry.write_text(json.dumps(registry, indent=2, sort_keys=True) + "\n")
        # gated is "human at work": one untracked file after the commit.
        (self.gated / "scratch.txt").write_text("uncommitted\n")
        self.hashes = self.snapshot()

    def snapshot(self):
        return {name: sha256_tree(path) for name, path in
                (("open-loop", self.open_loop), ("gated", self.gated), ("stopped", self.stopped),
                 ("commons", self.commons), ("home", self.home))}

    def _post(self, repo, kind, area, thread, agent, date, summary, **extra):
        ident = post_id(date, agent)
        target = repo / "docs" / "board" / area / thread
        target.mkdir(parents=True, exist_ok=True)
        (target / (ident + ".md")).write_text(post_text(kind, area, thread, agent, date, summary, **extra))
        return ident

    def _project(self, name, held_out, stop):
        repo = self.root / name
        (repo / "docs" / "board").mkdir(parents=True)
        (repo / "docs" / "board" / "README.md").write_text("# Board (fixture)\n")
        evo = repo / "docs" / "evolution"
        evo.mkdir()
        (evo / "constitution.md").write_text(CONSTITUTION_ON)
        (evo / "evaluators.md").write_text(EVALUATORS % held_out)
        (evo / "archive.md").write_text(ARCHIVE)
        if stop:
            (evo / "STOP").write_text("incident\n")
        p = lambda *a, **k: self._post(repo, *a, **k)  # noqa: E731
        # Handoff nobody picked up (open thread), and a handoff that a later claim picked up.
        self.ids[name + ":handoff"] = p("handoff", "governance", "left-open", self.A, "2026-09-02T09:00:00Z", "Stopped early - continue the export page")
        p("handoff", "governance", "picked-up", self.A, "2026-09-02T10:00:00Z", "Stopped early - parser")
        p("claim", "governance", "picked-up", self.B, "2026-09-03T10:00:00Z", "Continuing the parser", expires="2026-09-30T00:00:00Z")
        # Eligible suggestion (two distinct counted voters), one needing votes, one expired.
        s1 = p("suggestion", "evolution", "speedup", self.A, "2026-09-01T09:00:00Z", "Cache the parser table")
        p("vote", "evolution", "speedup", self.B, "2026-09-02T09:00:00Z", "up: fewer retries", on=s1, vote="up")
        p("vote", "evolution", "speedup", self.C, "2026-09-02T10:00:00Z", "up", on=s1, vote="up")
        self.ids[name + ":eligible"] = s1
        s2 = p("suggestion", "evolution", "fresh", self.B, "2026-09-08T09:00:00Z", "Split the long fixture")
        p("vote", "evolution", "fresh", self.C, "2026-09-09T09:00:00Z", "up", on=s2, vote="up")
        self.ids[name + ":needs_votes"] = s2
        self.ids[name + ":expired_suggestion"] = p("suggestion", "evolution", "lonely", self.D, "2026-08-01T09:00:00Z", "Rename the archive columns")
        # Unanswered question, and an answered one.
        self.ids[name + ":question"] = p("question", "governance", "how-to-tag", self.C, "2026-09-05T09:00:00Z", "How do we tag releases here?")
        q2 = p("question", "governance", "answered", self.C, "2026-09-05T10:00:00Z", "Which branch is main?")
        p("answer", "governance", "answered", self.D, "2026-09-05T11:00:00Z", "main is main", on=q2)
        # Claims: one active, one expired.
        p("claim", "governance", "active-claim", self.A, "2026-09-09T08:00:00Z", "Working on the roster", expires="2026-09-30T00:00:00Z")
        self.ids[name + ":stale_claim"] = p("claim", "governance", "old-claim", self.B, "2026-08-20T08:00:00Z", "Started the audit", expires="2026-08-27T00:00:00Z")
        # A proposal awaiting the human (open-loop only) and one already accepted (gated only).
        if name == "open-loop":
            self.ids["awaiting"] = p("answer", "governance", "one-proposal", self.A, "2026-09-06T09:00:00Z",
                                     "Agreed - one plain proposal", resolution="agreed")
        if name == "gated":
            ans = p("answer", "governance", "decided", self.A, "2026-09-06T09:00:00Z", "Agreed - move the button", resolution="agreed")
            p("result", "governance", "decided", self.A, "2026-09-06T10:00:00Z", "Human said yes", on=ans, resolution="accepted")
        git_init(repo)
        if name == "open-loop":
            subprocess.run(["git", "branch", "evolve/product/stray-branch"], cwd=str(repo), check=True, capture_output=True)
        return repo


class NudgeTestCase(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-nudge-")).resolve()
        cls.env_before = {k: os.environ.pop(k, None) for k in ("PULSE_HOME", "PULSE_COMMONS", "PULSE_DASHBOARD_HOME")}
        cls.org = Org(cls.tmp)
        os.environ["PULSE_HOME"] = str(cls.org.home)

    @classmethod
    def tearDownClass(cls):
        for key, value in cls.env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def run_nudge(self, *args):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = nudge.main(["--now", NOW] + list(args))
        self.assertEqual(code, 0)
        return buf.getvalue()

    def snapshot(self, *args):
        return json.loads(self.run_nudge("--json", *args))

    def test_writes_nothing_anywhere(self):
        before = self.org.snapshot()
        self.run_nudge("--agent", Org.D)
        self.run_nudge("--json", "--by", "age", "--limit", "3")
        self.assertEqual(self.org.snapshot(), before)
        self.assertEqual(before, self.org.hashes, "nothing changed since the fixture was built")
        for repo in (self.org.open_loop, self.org.stopped, self.org.commons):
            status = subprocess.run(["git", "status", "--porcelain"], cwd=str(repo), capture_output=True, text=True, check=True).stdout
            self.assertEqual(status, "", "%s stays clean" % repo.name)

    def test_for_the_human_lists_only_awaiting_proposals(self):
        snap = self.snapshot()
        self.assertEqual([(p["project"], p["thread"]) for p in snap["for_the_human"]], [("open-loop", "governance/one-proposal")])
        self.assertEqual(snap["for_the_human"][0]["read"], "docs/board/governance/one-proposal/%s.md" % self.org.ids["awaiting"])
        text = self.run_nudge()
        self.assertIn("FOR THE HUMAN", text)
        self.assertIn("Agreed - one plain proposal", text)
        self.assertNotIn("Agreed - move the button", text, "an accepted proposal is decided, not awaiting")

    def test_ranking_by_default(self):
        snap = self.snapshot()
        kinds = [w["kind"] for w in snap["work"]]
        self.assertEqual(kinds, sorted(kinds, key=lambda k: nudge.RANKS[k]), "ranked by bucket")
        by_kind = {}
        for w in snap["work"]:
            by_kind.setdefault(w["kind"], []).append((w["project"], w["thread"], w["id"]))
        # Handoffs: the open one in every project; the picked-up one never.
        self.assertEqual(sorted(by_kind["handoff"]), sorted(
            [(n, "governance/left-open", self.org.ids[n + ":handoff"]) for n in ("open-loop", "gated", "stopped")]))
        # Eligible: only where the loop gate is open; gated is listed as gated; stopped hides it.
        self.assertEqual(by_kind["eligible"], [("open-loop", "evolution/speedup", self.org.ids["open-loop:eligible"])])
        self.assertEqual(by_kind["eligible-gated"], [("gated", "evolution/speedup", self.org.ids["gated:eligible"])])
        self.assertNotIn(("stopped", "evolution/speedup", self.org.ids["stopped:eligible"]), by_kind.get("eligible", []) + by_kind["eligible-gated"])
        # Questions: the unanswered one per project plus the commons; the answered one never.
        self.assertEqual(sorted(by_kind["question"]), sorted(
            [(n, "governance/how-to-tag", self.org.ids[n + ":question"]) for n in ("open-loop", "gated", "stopped")]
            + [("commons", "questions/what-do-you-want", self.org.ids["commons_question"])]))
        # Stale claims: the expired one per project; the active one never.
        self.assertEqual(sorted(by_kind["stale-claim"]), sorted(
            [(n, "governance/old-claim", self.org.ids[n + ":stale_claim"]) for n in ("open-loop", "gated", "stopped")]))
        # Hygiene: the stray branch in open-loop and STOP in stopped.
        hygiene = {(w["project"], w["thread"]) for w in snap["work"] if w["kind"] == "hygiene"}
        self.assertEqual(hygiene, {("open-loop", "evolve/product/stray-branch"), ("stopped", "docs/evolution/STOP")})
        self.assertNotIn("ballot", by_kind, "no ballot without --agent")
        gated_reason = [p for p in snap["projects"] if p["slug"] == "gated"][0]["loop"]["reason"]
        self.assertEqual(gated_reason, "held-out set pending")
        eligible_gated = [w for w in snap["work"] if w["kind"] == "eligible-gated"][0]
        self.assertIn("held-out set pending", eligible_gated["next"])

    def test_ballot_with_agent(self):
        snap = self.snapshot("--agent", Org.D, "--project", "open-loop")
        ballot = {w["id"] for w in snap["work"] if w["kind"] == "ballot"}
        self.assertEqual(ballot, {self.org.ids["open-loop:eligible"], self.org.ids["open-loop:needs_votes"]},
                         "D wrote the expired one and has not voted on the other two; closed and expired never appear")
        snap_c = self.snapshot("--agent", Org.C, "--project", "open-loop")
        self.assertEqual({w["id"] for w in snap_c["work"] if w["kind"] == "ballot"}, set(),
                         "C voted on both open suggestions")

    def test_human_at_work_flag_and_project_lines(self):
        snap = self.snapshot()
        projects = {p["slug"]: p for p in snap["projects"]}
        self.assertTrue(projects["gated"]["human_at_work"])
        self.assertEqual(projects["gated"]["dirty_paths"], 1)
        self.assertFalse(projects["open-loop"]["human_at_work"])
        self.assertTrue(projects["open-loop"]["brain"])
        self.assertTrue(projects["open-loop"]["loop"]["open"])
        self.assertFalse(projects["stopped"]["loop"]["open"])
        self.assertEqual(projects["stopped"]["loop"]["reason"], "STOP present")
        self.assertFalse(projects["ghost"]["readable"])
        self.assertEqual(projects["ghost"]["error"], "path missing")
        self.assertEqual(projects["commons"]["kind"], "commons")
        for w in snap["work"]:
            self.assertEqual(w["human_at_work"], w["project"] == "gated", w)
        text = self.run_nudge()
        self.assertIn("human at work (1 path)", text)
        self.assertIn("unreadable: path missing", text)
        self.assertIn("[human at work]", text)
        self.assertIn("loop closed: STOP present", text)
        self.assertIn("* = brain", text)

    def test_filters_orderings_and_limit(self):
        only = self.snapshot("--project", "stopped")
        self.assertEqual({w["project"] for w in only["work"]}, {"stopped"})
        self.assertEqual([p["slug"] for p in only["projects"]], ["stopped"], "the commons joins only when asked or unfiltered")
        unknown = self.snapshot("--project", "nope")
        self.assertEqual(unknown["unknown_projects"], ["nope"])
        self.assertEqual(unknown["work"], [])
        by_project = self.snapshot("--by", "project")
        projects_seen = [w["project"] for w in by_project["work"]]
        self.assertEqual(projects_seen, sorted(projects_seen))
        by_age = self.snapshot("--by", "age")
        ages = [w["age_days"] if w["age_days"] is not None else -1 for w in by_age["work"]]
        self.assertEqual(ages, sorted(ages, reverse=True), "oldest first")
        text = self.run_nudge("--limit", "2")
        self.assertIn("showing 2", text)
        self.assertEqual(text.count("       next: "), 2)
        limited = self.snapshot("--limit", "2")
        self.assertEqual(len(limited["work"]), 2)

    def test_json_shape(self):
        snap = self.snapshot("--agent", Org.D)
        self.assertEqual(snap["version"], nudge.SNAPSHOT_VERSION)
        self.assertEqual(snap["source"], "markdown")
        self.assertEqual(snap["now"], NOW)
        self.assertEqual(snap["registry"], str(self.org.registry))
        self.assertIsNone(snap["registry_error"])
        self.assertEqual(snap["commons"], str(self.org.commons))
        for key in ("projects", "for_the_human", "work", "unknown_projects"):
            self.assertIsInstance(snap[key], list)
        for w in snap["work"]:
            for key in ("rank", "kind", "project", "thread", "id", "summary", "date", "age_days", "next", "tier", "human_at_work"):
                self.assertIn(key, w)
        text = self.run_nudge()
        for repo in (self.org.open_loop, self.org.gated, self.org.stopped, self.org.commons):
            self.assertNotIn(str(repo), text, "a readable project's path appears only with --json")
        self.assertIn(str(self.org.root / "nowhere"), text, "an unreadable entry shows its path so the registry can be repaired")

    def test_empty_and_missing_registry(self):
        empty = self.tmp / "empty.json"
        empty.write_text(json.dumps({"version": 1, "commons": "", "projects": []}))
        text = self.run_nudge("--registry", str(empty), "--home", str(self.tmp / "no-home"))
        self.assertIn("no projects registered", text)
        self.assertIn("pulse-dashboard.py register --repo PATH", text)
        text = self.run_nudge("--registry", str(self.tmp / "absent.json"), "--home", str(self.tmp / "no-home"))
        self.assertIn("no registry at", text)
        self.assertIn("none - nothing on any board asks for work", text)

    def test_store_source_preferred_when_present(self):
        root = Path(tempfile.mkdtemp(prefix="pulse-nudge-store-")).resolve()
        old_home = os.environ.get("PULSE_HOME")
        try:
            home = root / "home"
            repo = root / "stored"
            (repo / "docs" / "evolution").mkdir(parents=True)
            (repo / "AGENTS.md").write_text("# rules\n", encoding="utf-8")
            (repo / "README.md").write_text("stored\n", encoding="utf-8")
            (repo / "docs" / "evolution" / "constitution.md").write_text(CONSTITUTION_ON, encoding="utf-8")
            (repo / "docs" / "evolution" / "evaluators.md").write_text(EVALUATORS % "H01", encoding="utf-8")
            (repo / "docs" / "evolution" / "archive.md").write_text(ARCHIVE, encoding="utf-8")
            git_init(repo)
            os.environ["PULSE_HOME"] = str(home)
            con = pulse.connect(home, create=True)
            try:
                pulse.add_project(con, repo, slug="stored", brain=True, verify_cmd="true", actor="test")
                pulse.join_agent(con, "aurora", "runner-a", "model-a", "s-store")
                pulse.create_post(con, "question", "stored", "governance", "store-mode",
                                  "Does store mode work?", agent="aurora")
            finally:
                con.close()
            snap = self.snapshot("--project", "stored")
            self.assertEqual(snap["source"], "store")
            self.assertEqual([p["slug"] for p in snap["projects"]], ["stored"])
            self.assertEqual([(w["project"], w["kind"], w["thread"]) for w in snap["work"]],
                             [("stored", "question", "governance/store-mode")])
        finally:
            if old_home is None:
                os.environ.pop("PULSE_HOME", None)
            else:
                os.environ["PULSE_HOME"] = old_home
            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
