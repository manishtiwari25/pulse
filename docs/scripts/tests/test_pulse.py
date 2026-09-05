#!/usr/bin/env python3
"""Tests for docs/scripts/pulse.py (ADR-019): the agents' store and runner.

Run from the repository root:

    python3 -m unittest discover -s docs/scripts/tests -p 'test_pulse.py'

Every test pins PULSE_HOME to a throwaway directory, so nothing here touches
the real ~/.pulse store, the real board, or the real checkout. The tally
parity test reuses the dashboard's board fixture (one of every board
situation) and checks that `pulse records` prints the same typed records as
`board.sh --records`, agent names aside.
"""

import importlib.util
import io
import os
import shutil
import sqlite3
import stat
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
BOARD_SH = SCRIPTS / "board.sh"
NOW = "2026-09-10T12:00:00Z"


def load_module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


pulse = load_module("pulse", "pulse.py")
from test_pulse_dashboard import Fixture  # noqa: E402  (the shared board fixture)

_ENV_BEFORE = {}


def setUpModule():
    for key in ("PULSE_HOME", "PULSE_AGENT", "PULSE_SESSION", "PULSE_COMMONS"):
        _ENV_BEFORE[key] = os.environ.pop(key, None)


def tearDownModule():
    for key, value in _ENV_BEFORE.items():
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value


GIT_ENV = dict(GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false"] + list(args),
                          capture_output=True, text=True, env=dict(os.environ, **GIT_ENV))


def make_repo(root, name="proj"):
    repo = Path(root) / name
    repo.mkdir(parents=True)
    (repo / "AGENTS.md").write_text("# rules\n", encoding="utf-8")
    (repo / "README.md").write_text("hello\n", encoding="utf-8")
    subprocess.run(["git", "init", "-q", str(repo)], check=True, capture_output=True)
    git(repo, "checkout", "-q", "-B", "main")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "init")
    return repo


def run_cli(*args):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = pulse.main(list(args))
    return code, out.getvalue(), err.getvalue()


class StoreTestCase(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="pulse-store-test-")).resolve()
        self.home = self.tmp / "home"
        os.environ["PULSE_HOME"] = str(self.home)
        os.environ.pop("PULSE_AGENT", None)
        self.repo = make_repo(self.tmp)
        code, out, err = run_cli("init", "--path", str(self.repo), "--verify", "true")
        self.assertEqual(code, 0, err)
        self.con = pulse.connect()

    def tearDown(self):
        self.con.close()
        shutil.rmtree(self.tmp, ignore_errors=True)

    # -- store and projects -------------------------------------------------

    def test_init_registers_project_and_queues_onboarding(self):
        projects = pulse.project_rows(self.con)
        self.assertEqual([p["slug"] for p in projects], ["proj"])
        self.assertEqual(projects[0]["verify_cmd"], "true")
        jobs = self.con.execute("SELECT kind, status FROM jobs").fetchall()
        self.assertEqual([(j["kind"], j["status"]) for j in jobs], [("onboard", "queued")])
        # a second init refreshes instead of duplicating
        code, out, _ = run_cli("init", "--path", str(self.repo))
        self.assertEqual(code, 0)
        self.assertIn("refreshed", out)
        self.assertIn("already queued", out)
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM projects").fetchone()[0], 1)

    def test_store_never_lives_inside_a_project(self):
        os.environ["PULSE_HOME"] = str(self.repo / ".pulse-inside")
        with self.assertRaises(pulse.PulseError):
            con = pulse.connect(create=True)
            pulse.add_project(con, self.repo)

    def test_schema_version_recorded(self):
        row = self.con.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()
        self.assertEqual(int(row["value"]), pulse.SCHEMA_VERSION)
        self.assertIn("CREATE TABLE IF NOT EXISTS posts", pulse.schema_text())

    # -- agents -------------------------------------------------------------

    def test_agent_names_are_unique_and_never_reused(self):
        row, created = pulse.join_agent(self.con, "aurora", "copilot-cli", "model-a", "s1")
        self.assertTrue(created)
        row, created = pulse.join_agent(self.con, "aurora", "copilot-cli", "model-a", "s2")
        self.assertFalse(created)
        self.assertIn("s2", row["sessions"])
        with self.assertRaises(pulse.PulseError):
            pulse.join_agent(self.con, "aurora", "other-runner", "model-b")
        with self.assertRaises(pulse.PulseError):
            pulse.join_agent(self.con, "Bad Name!", "r", "m")
        with self.assertRaises(pulse.PulseError):
            pulse.join_agent(self.con, "human", "r", "m")
        # a name the migration created from history becomes live when its owner joins with it
        legacy, _ = pulse.join_agent(self.con, "ember", "copilot-cli", "model-a", kind="legacy", legacy_id="copilot-cli_model-a")
        self.assertEqual(legacy["kind"], "legacy")
        live, created = pulse.join_agent(self.con, "ember", "copilot-cli", "model-a", "s3")
        self.assertFalse(created)
        self.assertEqual(live["kind"], "session")
        self.assertEqual(live["legacy_id"], "copilot-cli_model-a")
        spawned = pulse.spawn_agent(self.con, "copilot", "m")
        self.assertNotEqual(spawned["name"], "aurora")
        self.assertIn(spawned["name"], pulse.NAME_WORDS)
        # prune, then the name is gone for good
        pruned = pulse.prune_agents(self.con, now="2027-01-01T00:00:00Z")
        self.assertIn("aurora", [n for n, _ in pruned])
        with self.assertRaises(pulse.PulseError):
            pulse.join_agent(self.con, "aurora", "copilot-cli", "model-a")
        with self.assertRaises(pulse.PulseError):
            pulse.resolve_agent(self.con, "aurora")
        self.assertEqual(pulse.agent_row(self.con, "human")["status"], "active")

    def test_prune_spares_useful_and_busy_agents(self):
        pulse.join_agent(self.con, "busy", "r", "m")
        pulse.join_agent(self.con, "useful", "r", "m")
        pulse.join_agent(self.con, "idle", "r", "m")
        pulse.create_post(self.con, "finding", "proj", "governance", "t", "a finding", "x", agent="useful")
        job, _ = pulse.add_job(self.con, "proj", "task", "hold", created_by="test")
        self.con.execute("UPDATE jobs SET status = 'leased', lease_agent = 'busy', lease_until = '2099-01-01T00:00:00Z' WHERE id = ?", (job["id"],))
        later = "2026-10-01T00:00:00Z"
        self.con.execute("UPDATE agents SET last_useful_at = ? WHERE name = 'useful'", (later,))
        pruned = dict(pulse.prune_agents(self.con, now="2026-10-02T00:00:00Z"))
        self.assertIn("idle", pruned)
        self.assertNotIn("busy", pruned)
        self.assertNotIn("useful", pruned)

    # -- posts --------------------------------------------------------------

    def test_posts_are_immutable_and_events_append_only(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        post = pulse.create_post(self.con, "finding", "proj", "governance", "immutability", "found it", "body", agent="aurora")
        with self.assertRaises(sqlite3.IntegrityError):
            self.con.execute("UPDATE posts SET summary = 'edited' WHERE id = ?", (post["id"],))
        with self.assertRaises(sqlite3.IntegrityError):
            self.con.execute("DELETE FROM posts WHERE id = ?", (post["id"],))
        with self.assertRaises(sqlite3.IntegrityError):
            self.con.execute("DELETE FROM events")
        self.assertEqual(pulse.post_row(self.con, post["id"])["summary"], "found it")
        self.assertEqual(pulse.agent_row(self.con, "aurora")["posts"], 1)

    def test_votes_need_a_target_and_refuse_self_votes(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        pulse.join_agent(self.con, "cinder", "r", "m")
        sug = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the table", "one variable", agent="aurora")
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "no target", agent="cinder", vote="up")
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "self", agent="aurora", on=sug["id"], vote="up")
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "no direction", agent="cinder", on=sug["id"])
        v = pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: fewer retries", agent="cinder", on=sug["id"], vote="up")
        self.assertEqual(v["vote"], "up")
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "finding", "proj", "evolution", "speed", "bad", agent="cinder", resolution="whatever")

    def test_amendment_needs_both_blocks_and_votes_need_hash_and_read_line(self):
        pulse.join_agent(self.con, "aurora", "r", "m", "s-a")
        pulse.join_agent(self.con, "cinder", "r", "m", "s-c")
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "amendment", "proj", "governance", "amend", "no blocks", "just words", agent="aurora")
        body = "## Current text\n\n```\n| Quorum | 2 |\n```\n\n## Proposed text\n\n```\n| Quorum | 3 |\n```\n"
        am = pulse.create_post(self.con, "amendment", "proj", "governance", "amend", "raise quorum", body, agent="aurora")
        self.assertEqual(am["text_hash"], pulse.block_hash(["| Quorum | 3 |"]))
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "vote", "proj", "governance", "amend", "up", "Read: raises quorum", agent="cinder", on=am["id"], vote="up")
        with self.assertRaises(pulse.PulseError):
            pulse.create_post(self.con, "vote", "proj", "governance", "amend", "up", "no read line", agent="cinder", on=am["id"], vote="up",
                              text_hash=am["text_hash"])
        v = pulse.create_post(self.con, "vote", "proj", "governance", "amend", "up", "up\n\nRead: raises the quorum to 3.", agent="cinder",
                              on=am["id"], vote="up", text_hash=am["text_hash"])
        self.assertEqual(v["has_read_line"], 1)
        code, out, _ = run_cli("hash", am["id"])
        self.assertEqual(out.strip(), am["text_hash"])

    def test_post_cli_uses_identity_from_environment(self):
        run_cli("agent", "join", "--name", "aurora", "--runner", "r", "--model", "m")
        code, out, err = run_cli("post", "finding", "--project", "proj", "--area", "governance", "--thread", "cli", "--summary", "via cli")
        self.assertEqual(code, 2, "no identity must fail")
        os.environ["PULSE_AGENT"] = "aurora"
        code, out, err = run_cli("post", "finding", "--project", "proj", "--area", "governance", "--thread", "cli", "--summary", "via cli",
                                 "--body", "hello")
        self.assertEqual(code, 0, err)
        self.assertTrue(out.strip().endswith("-aurora"))
        code, out, _ = run_cli("show", out.strip())
        self.assertIn("summary: via cli", out)
        self.assertIn("hello", out)

    # -- digest, decisions ------------------------------------------------

    def test_proposal_waits_for_the_human_and_decide_records_once(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        prop = pulse.create_post(self.con, "opinion", "proj", "governance", "plan", "we agree: do X", "the agents agreed", agent="aurora",
                                 resolution="agreed")
        text = pulse.digest(self.con, "proj", now=NOW)
        self.assertIn("awaiting", text)
        self.assertIn("1 proposal awaiting", text)
        result = pulse.decide(self.con, prop["id"], "yes", "go ahead")
        self.assertEqual(result["resolution"], "accepted")
        self.assertEqual(result["agent"], "human")
        self.assertEqual(result["on_post"], prop["id"])
        with self.assertRaises(pulse.PulseError):
            pulse.decide(self.con, prop["id"], "no")
        t, _ = pulse.tally_for(self.con, "proj", None, NOW)
        self.assertEqual(t.proposal_status("governance/plan"), "accepted")
        self.assertEqual(self.con.execute("SELECT answer FROM decisions").fetchone()["answer"], "yes")
        code, out, _ = run_cli("proposals")
        self.assertIn("accepted", out)

    def test_digest_lists_everything_an_agent_needs(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        pulse.create_post(self.con, "claim", "proj", "governance", "work", "building X", agent="aurora", expires="2026-09-11T12:00:00Z")
        text = pulse.digest(self.con, "proj", now=NOW, agent="aurora")
        for heading in ("NOTICES", "ACTIVE CLAIMS", "OPEN THREADS", "PROPOSALS FOR THE HUMAN", "EXPERIMENT QUEUE", "AMENDMENTS",
                        "STALE", "JOBS", "AGENTS"):
            self.assertIn(heading, text)
        self.assertIn("governance/work", text)
        self.assertIn("onboard", text)
        self.assertIn("aurora", text)

    # -- jobs, leases, runner ---------------------------------------------

    def test_lease_is_exclusive_per_project_and_expires(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        pulse.join_agent(self.con, "cinder", "r", "m")
        job = pulse.lease_job(self.con, "aurora", now=NOW)
        self.assertEqual(job["kind"], "onboard")
        self.assertEqual(job["status"], "leased")
        pulse.add_job(self.con, "proj", "task", "second", created_by="test")
        self.assertIsNone(pulse.lease_job(self.con, "cinder", now=NOW), "one leased job per project")
        later = "2026-09-10T13:00:00Z"
        job2 = pulse.lease_job(self.con, "cinder", now=later)
        self.assertIsNotNone(job2, "an expired lease is requeued and retaken")
        self.assertEqual(job2["id"], job["id"])
        self.assertEqual(job2["generation"], 2)
        self.assertEqual(job2["lease_agent"], "cinder")
        with self.assertRaises(pulse.PulseError):
            pulse.finish_job(self.con, job2["id"], "aurora", "done")
        done = pulse.finish_job(self.con, job2["id"], "cinder", "done", "ok")
        self.assertEqual(done["status"], "done")

    def test_tier_two_jobs_are_refused(self):
        with self.assertRaises(pulse.PulseError):
            pulse.add_job(self.con, "proj", "task", "migrate the database", tier=2)

    def worker_script(self, body):
        script = self.tmp / "worker.sh"
        script.write_text("#!/bin/sh\nset -e\n" + body + "\n", encoding="utf-8")
        script.chmod(script.stat().st_mode | stat.S_IEXEC)
        return str(script)

    def test_work_lands_commits_on_a_local_branch_without_touching_the_checkout(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'echo "changed by $PULSE_AGENT" >> README.md\n'
            'git -c user.name=w -c user.email=w@x -c commit.gpgsign=false commit -qam "worker change"\n'
            'printf "SUMMARY: appended one line\\nFINDING: the README was short\\n" > PULSE_RESULT.md')
        before = git(self.repo, "rev-parse", "HEAD").stdout.strip()
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertEqual(run["commits"], 1)
        branch = "pulse/aurora/%s" % run["job_id"]
        self.assertEqual(run["branch"], branch)
        self.assertEqual(git(self.repo, "rev-parse", "HEAD").stdout.strip(), before, "HEAD of the real checkout unchanged")
        self.assertEqual(git(self.repo, "status", "--porcelain").stdout, "", "worktree untouched")
        self.assertEqual(git(self.repo, "rev-parse", "--verify", "refs/heads/%s" % branch).returncode, 0)
        self.assertIn("changed by aurora", git(self.repo, "show", "%s:README.md" % branch).stdout)
        self.assertFalse((self.home / "work" / run["job_id"]).exists(), "clone removed")
        self.assertTrue(Path(run["log"]).is_file())
        results = self.con.execute("SELECT * FROM posts WHERE kind = 'result'").fetchall()
        self.assertEqual(len(results), 1)
        self.assertIn("appended one line", results[0]["summary"])
        self.assertIn("the README was short", results[0]["body"])
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_done"], 1)
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "done")
        self.assertEqual(job["branch"], branch)

    def test_work_refuses_when_verify_fails_or_stopped_or_capped(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        self.con.execute("UPDATE projects SET verify_cmd = 'false' WHERE slug = 'proj'")
        worker = self.worker_script(
            'echo broken >> README.md\n'
            'git -c user.name=w -c user.email=w@x -c commit.gpgsign=false commit -qam "bad change"')
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "failed")
        self.assertEqual(run["branch"], "")
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "nothing landed")
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_failed"], 1)
        # the kill switch
        pulse.add_job(self.con, "proj", "task", "again", created_by="test")
        pulse.set_setting(self.con, "stop", "1", "human")
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNone(run)
        self.assertIn("stopped", reason)
        pulse.set_setting(self.con, "stop", "0", "human")
        # the daily cap counts real runs started today
        pulse.set_setting(self.con, "jobs_per_day", "1", "human")
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW[:10] + "T13:00:00Z")
        self.assertIsNone(run)
        self.assertIn("daily cap", reason)

    def test_worker_that_only_talks_is_a_failed_run(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('echo "Tier 0 - I would start by reading AGENTS.md"')  # exits 0, acts on nothing
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "failed")
        self.assertIn("wrote no PULSE_RESULT.md", run["summary"])
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_failed"], 1)
        brief = (self.home / "work" / ("%s.brief.md" % run["job_id"])).read_text()
        self.assertIn("This is your only run", brief)

    def test_work_times_out_and_dry_run_keeps_the_job(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        pulse.set_setting(self.con, "minutes_per_job", "0", "human")  # 0 minutes -> immediate timeout
        # sqlite stores minutes as text; a zero cap means the worker gets no time
        worker = self.worker_script("sleep 5")
        # a zero timeout raises immediately in subprocess; emulate with a tiny fraction via the setting floor
        self.con.execute("UPDATE settings SET value = '0' WHERE name = 'minutes_per_job'")
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIn(run["status"], ("timeout", "failed"))
        pulse.set_setting(self.con, "minutes_per_job", "10", "human")
        pulse.add_job(self.con, "proj", "task", "dry", created_by="test")
        run, _ = pulse.work_once(self.con, "aurora", dry_run=True, now=NOW)
        self.assertEqual(run["status"], "dry-run")
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE title = 'dry'").fetchone()["status"], "queued")
        self.assertTrue((self.home / "work" / ("%s.brief.md" % run["job_id"])).is_file())

    def test_worker_question_becomes_a_question_post(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('printf "SUMMARY: stopped\\nQUESTION: this needs a Tier 2 decision about billing\\n" > PULSE_RESULT.md')
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "done")
        self.assertEqual(run["commits"], 0)
        kinds = sorted(r["kind"] for r in self.con.execute("SELECT kind FROM posts"))
        self.assertEqual(kinds, ["question", "result"])

    def test_plan_creates_improve_answer_then_reflect_jobs(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        pulse.join_agent(self.con, "cinder", "r", "m")
        pulse.join_agent(self.con, "delta", "r", "m")
        pulse.finish_job(self.con, self.con.execute("SELECT id FROM jobs").fetchone()["id"], "human", "cancelled")
        sug = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the table", "one variable", agent="aurora", date="2026-09-09T09:00:00Z")
        pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up", agent="cinder", on=sug["id"], vote="up", date="2026-09-09T10:00:00Z")
        pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up", agent="delta", on=sug["id"], vote="up", date="2026-09-09T11:00:00Z")
        pulse.create_post(self.con, "question", "proj", "governance", "how", "where do votes live?", "asking", agent="delta", date="2026-09-09T12:00:00Z")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["kind"] for j in jobs], ["improve"])
        self.assertEqual(jobs[0]["source"], "suggestion:%s" % sug["id"])
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now=NOW), [], "one job at a time per project")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "done")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["kind"] for j in jobs], ["improve"], "the eligible suggestion is still open, so it is planned again")
        pulse.create_post(self.con, "result", "proj", "evolution", "speed", "tested and kept", "row A-1", agent="aurora", resolution="tested")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "done")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["kind"] for j in jobs], ["answer"])
        pulse.finish_job(self.con, jobs[0]["id"], "human", "cancelled")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["kind"] for j in jobs], ["reflect"], "a question gets one job ever; a cancelled attempt is not retried")
        pulse.create_post(self.con, "answer", "proj", "governance", "how", "in the store", "rows", agent="aurora", resolution="answered")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "done")
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now=NOW), [], "one reflect per day")

    def test_daemon_runs_one_loop_and_stops_on_the_switch(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script('printf "SUMMARY: looked around\\n" > PULSE_RESULT.md')
        out = io.StringIO()
        result = pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=1, out=out)
        self.assertEqual(result, "done")
        self.assertIn("done", out.getvalue())
        pulse.set_setting(self.con, "stop", "1", "human")
        out = io.StringIO()
        self.assertEqual(pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=1, out=out), "stopped")

    # -- export -----------------------------------------------------------

    def test_export_refuses_projects_and_overwrites(self):
        with self.assertRaises(pulse.PulseError):
            pulse.export_store(self.con, sql_path=self.repo / "dump.sql")
        target = self.tmp / "out" / "dump.sql"
        pulse.export_store(self.con, sql_path=target)
        self.assertIn("CREATE TABLE", target.read_text())
        with self.assertRaises(pulse.PulseError):
            pulse.export_store(self.con, sql_path=target)
        db = self.tmp / "out" / "copy.sqlite"
        pulse.export_store(self.con, db_path=db)
        con = sqlite3.connect(str(db))
        self.assertEqual(con.execute("SELECT COUNT(*) FROM projects").fetchone()[0], 1)
        con.close()


class MigrationTestCase(unittest.TestCase):
    """The Markdown board becomes rows; the tally stays identical to board.sh."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-migrate-test-")).resolve()
        cls.home = cls.tmp / "home"
        os.environ["PULSE_HOME"] = str(cls.home)
        cls.fx = Fixture(cls.tmp)
        cls.con = pulse.connect(create=True)
        cls.report = pulse.migrate(cls.con, repo=cls.fx.repo)
        cls.slug = cls.report["board"][0]

    @classmethod
    def tearDownClass(cls):
        cls.con.close()
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def normalized(self, text, legacy):
        agent_col = {"post": 6, "claim": 3, "proposal": 3, "svote": 2, "suggestion": 9, "amendment": 16, "avote": 3}
        rows = set()
        for line in text.splitlines():
            f = line.split("\t")
            if f[0] == "param" and f[1] == "repo":
                continue
            if f[0] == "post":
                f[1] = "<path>"
            idx = agent_col.get(f[0])
            if idx is not None and f[idx] in legacy:
                f[idx] = legacy[f[idx]]
            rows.add("\t".join(f))
        return rows

    def test_records_match_board_sh(self):
        legacy = {r["legacy_id"]: r["name"] for r in self.con.execute("SELECT legacy_id, name FROM agents WHERE legacy_id != ''")}
        for area in (None, "evolution", "governance"):
            args = ["--repo", str(self.fx.repo), "--now", NOW, "--records"] + (["--area", area] if area else [])
            expected = subprocess.run(["bash", str(BOARD_SH)] + args, capture_output=True, text=True, check=True).stdout
            got = pulse.snapshot_records(self.con, self.slug, area, NOW)
            self.assertEqual(self.normalized(expected, legacy), self.normalized(got, {}), "area %s" % area)

    def test_migration_is_idempotent_and_names_agents(self):
        counts = self.report["board"][1]
        self.assertEqual(counts["posts"], 19)
        self.assertEqual(counts["quarantined"], 0)
        again = pulse.import_board(self.con, self.fx.repo, self.slug)
        self.assertEqual(again["posts"], 0)
        self.assertEqual(again["skipped"], 19)
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM posts").fetchone()[0], 19)
        names = {r["legacy_id"]: r["name"] for r in self.con.execute("SELECT legacy_id, name FROM agents WHERE kind = 'legacy'")}
        self.assertEqual(names["runner-a/model-a"], "model-a")
        self.assertEqual(len(set(names.values())), len(names), "unique names")
        post = pulse.post_row(self.con, self.fx.ids["eligible"])
        self.assertEqual(post["origin"], "markdown:docs/board/evolution/speedup/%s.md" % self.fx.ids["eligible"])
        self.assertEqual(post["agent"], "model-a")

    def test_dashboard_shapes(self):
        rows = pulse.post_rows(self.con, self.slug)
        self.assertEqual(len(rows), 19)
        fm, body = rows[0]
        for key in ("kind", "agent", "date", "summary", "id", "area", "thread", "path"):
            self.assertIn(key, fm)
        self.assertTrue(fm["path"].startswith("store:"))
        self.assertTrue(pulse.store_fingerprint().startswith("store:"))
        reg = pulse.registry_from_store(self.con)
        self.assertEqual([p["slug"] for p in reg["projects"]], [self.slug])
        self.assertIsNotNone(pulse.project_for_repo(self.con, self.fx.repo / "docs"))

    def test_dashboard_builds_its_projection_from_the_store(self):
        dash = load_module("pulse_dashboard_under_test", "pulse-dashboard.py")
        self.assertTrue(dash.store_available())
        self.assertEqual(dash.store_slug_for(self.fx.repo), self.slug)
        self.assertEqual([p["slug"] for p in dash.load_registry(self.home)["projects"]], [self.slug])
        state_dir = self.tmp / "state"
        db = dash.build_projection(self.fx.repo, state_dir, now=NOW)
        con = sqlite3.connect(str(db))
        try:
            meta = {r[0]: r[1] for r in con.execute("SELECT key, value FROM metadata")}
            self.assertEqual(meta["tally_mode"], "store")
            self.assertEqual(con.execute("SELECT COUNT(*) FROM posts").fetchone()[0], 19)
            self.assertEqual(con.execute("SELECT COUNT(*) FROM suggestions WHERE status = 'eligible'").fetchone()[0], 1)
            path = con.execute("SELECT rel_path FROM posts LIMIT 1").fetchone()[0]
            self.assertTrue(path.startswith("store:"))
        finally:
            con.close()
        current, _ = dash.projection_is_current(self.fx.repo, db)
        self.assertTrue(current)
        # any new event in the store changes the fingerprint, so the projection rebuilds
        pulse.event(self.con, "test", "touch", "fingerprint", "")
        current, _ = dash.projection_is_current(self.fx.repo, db)
        self.assertFalse(current)

    def test_queue_and_ballot_after_migration(self):
        t, params = pulse.tally_for(self.con, self.slug, None, NOW)
        queue = {s["id"]: s for s in t.suggestion_records()}
        self.assertEqual(queue[self.fx.ids["eligible"]]["status"], "eligible")
        self.assertEqual(queue[self.fx.ids["eligible"]]["score"], 1)
        self.assertEqual(queue[self.fx.ids["needs_votes"]]["status"], "needs votes")
        self.assertEqual(queue[self.fx.ids["expired"]]["status"], "expired")
        amends, _ = t.amendment_records()
        self.assertEqual(amends[0]["hash"], self.fx.amendment_hash)
        self.assertEqual(amends[0]["voters"], 1)
        self.assertTrue(amends[0]["status"].startswith("passed"), amends[0]["status"])
        sug, am = pulse.ballot_items(self.con, self.slug, "model-b", NOW)
        self.assertNotIn(self.fx.ids["eligible"], [s["id"] for s in sug], "model-b already voted")
        self.assertNotIn(self.fx.ids["needs_votes"], [s["id"] for s in sug], "model-b wrote it")
        self.assertEqual(len(am), 1)


if __name__ == "__main__":
    unittest.main()
