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

import base64
import importlib.util
import io
import json
import os
import shlex
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import unittest
import unittest.mock
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
    for key in ("PULSE_HOME", "PULSE_AGENT", "PULSE_SESSION", "PULSE_PROJECT", "PULSE_COMMONS"):
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


class StoreFixture(unittest.TestCase):
    """A fresh store and one registered project; no tests of its own, so other test modules can build on it
    (`import test_pulse as tp`; subclass `tp.StoreFixture`) without re-running the store tests."""

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

    def worker_script(self, body, name="worker.sh"):
        script = self.tmp / name
        script.write_text("#!/bin/sh\nset -e\n" + body + "\n", encoding="utf-8")
        script.chmod(script.stat().st_mode | stat.S_IEXEC)
        return str(script)

    def signature_status(self, ref, agent, pubkey):
        """git's %G? for the commit at `ref` under a one-line allowed-signers file for `pubkey` (ADR-022)."""
        signers = self.tmp / ("signers-%s" % agent)
        signers.write_text('%s@pulse.local namespaces="git" %s\n' % (agent, pubkey), encoding="utf-8")
        out = subprocess.run(["git", "-C", str(self.repo), "-c", "gpg.ssh.program=ssh-keygen", "-c", "gpg.ssh.allowedSignersFile=%s" % signers,
                              "log", "-1", "--format=%G?", ref], capture_output=True, text=True)
        return out.stdout.strip()


class StoreTestCase(StoreFixture):
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

    def test_a_yes_queues_one_job_in_the_proposals_project(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        prop = pulse.create_post(self.con, "answer", "proj", "runner", "environment", "Agreed: accept ADR-026", "Proposal: do X.",
                                 agent="aurora", resolution="agreed")
        before = self.con.execute("SELECT COUNT(*) FROM jobs").fetchone()[0]
        result = pulse.decide(self.con, prop["id"], "yes", "go ahead")
        job = self.con.execute("SELECT * FROM jobs WHERE source = ?", ("decide:%s" % prop["id"],)).fetchone()
        self.assertIsNotNone(job)
        self.assertEqual((job["kind"], job["tier"], job["priority"], job["status"], job["created_by"]), ("task", 1, 40, "queued", "human"))
        self.assertIn("Act on the human's yes", job["title"])
        for needle in ("answered YES", prop["id"], result["id"], "go ahead", "Proposal: do X.", "never crossed"):
            self.assertIn(needle, job["brief"])
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM jobs").fetchone()[0], before + 1)
        self.assertTrue(self.con.execute("SELECT 1 FROM events WHERE action = 'decide.job' AND subject = ?", (job["id"],)).fetchone())
        # a no queues nothing; a proposal that authorizes a command (ADR-021 marker) queues nothing
        declined = pulse.create_post(self.con, "answer", "proj", "runner", "environment", "Agreed: do Y", "Proposal: Y.", agent="aurora",
                                     resolution="agreed")
        pulse.decide(self.con, declined["id"], "no", "not now")
        gate = pulse.create_post(self.con, "answer", "proj", "governance", "pair", "Agreed: pair hands-2", "Proposal.\n\npair: hands-2\n",
                                 agent="aurora", resolution="agreed")
        pulse.decide(self.con, gate["id"], "yes")
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM jobs").fetchone()[0], before + 1)

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

    def test_the_decision_gate_lets_the_human_through_and_an_agent_only_with_a_fresh_yes(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        self.assertEqual(pulse.agent_gate(self.con, "pair: hands-2", "human", now=NOW), "")
        self.assertEqual(pulse.agent_gate(self.con, "pair: hands-2", "", now=NOW), "")
        with self.assertRaises(pulse.PulseError):
            pulse.agent_gate(self.con, "pair: hands-2", "aurora", now=NOW)
        prop = pulse.create_post(self.con, "answer", "proj", "runner", "pair", "we agree: pair hands-2",
                                 "The agents agreed.\n\npair: hands-2\n", agent="aurora", resolution="agreed")
        with self.assertRaises(pulse.PulseError):   # proposed, not yet decided
            pulse.agent_gate(self.con, "pair: hands-2", "aurora", now=NOW)
        pulse.decide(self.con, prop["id"], "yes", "go ahead")
        decided_at = self.con.execute("SELECT decided_at FROM decisions").fetchone()["decided_at"]
        self.assertEqual(pulse.agent_gate(self.con, "pair: hands-2", "aurora", now=decided_at), prop["id"])
        self.assertEqual(pulse.agent_gate(self.con, "pair: hands-2", "aurora", now=decided_at), prop["id"])
        for other in ("pair: hands-3", "onboard: hands-2", "pair:hands-2"):
            with self.assertRaises(pulse.PulseError):
                pulse.agent_gate(self.con, other, "aurora", now=decided_at)
        eight_days = (pulse.parse_iso(decided_at) + pulse._dt.timedelta(days=8)).strftime("%Y-%m-%dT%H:%M:%SZ")
        with self.assertRaises(pulse.PulseError):
            pulse.agent_gate(self.con, "pair: hands-2", "aurora", now=eight_days)
        later = pulse.create_post(self.con, "answer", "proj", "runner", "pair", "we agree: do not pair hands-2",
                                  "Second thoughts.\n\npair: hands-2\n", agent="aurora", resolution="agreed")
        pulse.decide(self.con, later["id"], "no", "not now")
        with self.assertRaises(pulse.PulseError):   # the newest decision wins, and it is a no
            pulse.agent_gate(self.con, "pair: hands-2", "aurora", now=decided_at)

    def test_tier_two_jobs_are_refused(self):
        with self.assertRaises(pulse.PulseError):
            pulse.add_job(self.con, "proj", "task", "migrate the database", tier=2)

    def test_work_lands_commits_on_a_local_branch_without_touching_the_checkout(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'echo "changed by $PULSE_AGENT" >> README.md\n'
            'git commit -qam "worker change"\n'
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
        self.assertFalse((self.home / "work" / ("%s.key" % run["job_id"])).exists(), "the run's private key is removed with it")
        self.assertTrue(Path(run["log"]).is_file())
        # ADR-022: the commit is the agent's, signed by this run's key, and says which job and hands made it
        self.assertTrue(run["signing_pubkey"].startswith("ssh-ed25519 "), run["signing_pubkey"])
        self.assertEqual(self.signature_status(branch, "aurora", run["signing_pubkey"]), "G")
        body = git(self.repo, "log", "-1", "--format=%an <%ae>%n%(trailers)", branch).stdout
        self.assertIn("aurora <aurora@pulse.local>", body)
        self.assertIn("Pulse-Agent: aurora", body)
        self.assertIn("Pulse-Job: %s" % run["job_id"], body)
        self.assertIn("Pulse-Executor: local", body)
        results = self.con.execute("SELECT * FROM posts WHERE kind = 'result'").fetchall()
        self.assertEqual(len(results), 1)
        self.assertIn("appended one line", results[0]["summary"])
        self.assertIn("the README was short", results[0]["body"])
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_done"], 1)
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "done")
        self.assertEqual(job["branch"], branch)

    def test_work_drops_a_committed_result_file_before_landing(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'echo "changed by $PULSE_AGENT" >> README.md\n'
            'printf "SUMMARY: appended and committed the result file too\\n" > PULSE_RESULT.md\n'
            'git add -A && git commit -qm "worker change"')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        branch = "pulse/aurora/%s" % run["job_id"]
        self.assertEqual(run["commits"], 2, "the worker's commit plus the runner's removal")
        self.assertNotEqual(git(self.repo, "cat-file", "-e", "%s:PULSE_RESULT.md" % branch).returncode, 0,
                            "PULSE_RESULT.md is not on the landed branch")
        self.assertIn("changed by aurora", git(self.repo, "show", "%s:README.md" % branch).stdout)

    def test_work_never_lands_commits_that_touch_owner_owned_paths(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'mkdir -p docs/evolution .github/workflows\n'
            'echo "baseline: B-999" > docs/evolution/evaluators.md\n'
            'echo "on: push" > .github/workflows/ci.yml\n'
            'git add -A && git commit -qm "owner files"\n'
            'printf "SUMMARY: edited the gates\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "failed")
        self.assertEqual(run["branch"], "")
        self.assertIn("owner-owned paths", run["summary"])
        self.assertIn("docs/evolution/evaluators.md", run["summary"])
        self.assertIn(".github/workflows/ci.yml", run["summary"])
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "nothing landed")

    def test_work_refuses_when_verify_fails_or_stopped_or_capped(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        self.con.execute("UPDATE projects SET verify_cmd = 'false' WHERE slug = 'proj'")
        worker = self.worker_script(
            'echo broken >> README.md\n'
            'git commit -qam "bad change"')
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
        self.assertEqual([j["kind"] for j in jobs], ["answer"], "a built suggestion is never planned again, even while its thread is open")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "cancelled")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["kind"] for j in jobs], ["reflect"], "a question gets one job ever; a cancelled attempt is not retried")
        pulse.create_post(self.con, "answer", "proj", "governance", "how", "in the store", "rows", agent="aurora", resolution="answered")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "done")
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now=NOW), [], "one reflect per day")

    def test_plan_ballot_job_when_only_below_quorum_cards_wait(self):
        """WU-015: idle hands vote. Workers vote only at the end of a job, so with nothing eligible and no open
        question the planner makes one ballot job for cards older than BALLOT_HOURS - and never a second within it."""
        pulse.join_agent(self.con, "aurora", "r", "m")
        pulse.join_agent(self.con, "cinder", "r", "m")
        pulse.join_agent(self.con, "delta", "r", "m")
        pulse.finish_job(self.con, self.con.execute("SELECT id FROM jobs").fetchone()["id"], "human", "cancelled")
        old = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the table",
                                "one variable\nSignal: check:tests", agent="aurora", date="2026-09-10T05:00:00Z")
        young = pulse.create_post(self.con, "suggestion", "proj", "evolution", "colour", "tint the header",
                                  "fresh card", agent="cinder", date="2026-09-10T09:30:00Z")
        pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up", agent="cinder", on=old["id"], vote="up",
                          date="2026-09-10T06:00:00Z")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([(j["kind"], j["tier"]) for j in jobs], [("ballot", 0)])
        self.assertEqual(jobs[0]["source"], "ballot:proj:20260910T120000Z")
        self.assertIn(old["id"], jobs[0]["brief"], "the seven-hour-old card at one voter of two is on the ballot")
        self.assertNotIn(young["id"], jobs[0]["brief"], "a card younger than six hours is left for its own session")
        self.assertIn("make no commits", jobs[0]["brief"])
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now=NOW), [], "one job at a time per project")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "failed")
        jobs = pulse.plan_jobs(self.con, "proj", now="2026-09-10T15:00:00Z")
        self.assertEqual([j["kind"] for j in jobs], ["reflect"],
                         "no second ballot job within six hours, whatever became of the first; the day's reflect follows")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "done")
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now="2026-09-10T17:59:00Z"), [])
        jobs = pulse.plan_jobs(self.con, "proj", now="2026-09-10T18:01:00Z")
        self.assertEqual([j["kind"] for j in jobs], ["ballot"], "six hours on, the still-waiting cards get another ballot job")
        self.assertIn(young["id"], jobs[0]["brief"], "the once-young card is old enough now")
        pulse.finish_job(self.con, jobs[0]["id"], "human", "done")
        pulse.create_post(self.con, "vote", "proj", "evolution", "colour", "up", agent="aurora", on=young["id"], vote="up",
                          date="2026-09-10T18:30:00Z")
        pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up", agent="delta", on=old["id"], vote="up",
                          date="2026-09-10T18:30:00Z")
        jobs = pulse.plan_jobs(self.con, "proj", now="2026-09-11T01:00:00Z")
        self.assertEqual([j["kind"] for j in jobs], ["improve"], "a card the ballot carried to quorum is built before any ballot")
        self.assertEqual(jobs[0]["source"], "suggestion:%s" % old["id"])

    def test_ballot_job_brief_carries_each_card_body(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.join_agent(self.con, "cinder", "r", "m")
        pulse.finish_job(self.con, self.con.execute("SELECT id FROM jobs").fetchone()["id"], "human", "cancelled")
        card = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the table",
                                 "The body says why.\nSignal: check:tests", agent="cinder", date="2026-09-10T05:00:00Z")
        job = pulse.plan_jobs(self.con, "proj", now=NOW)[0]
        self.assertEqual(job["kind"], "ballot")
        worker = self.worker_script('printf "SUMMARY: voted once\\nVOTE: %s up a real check would move\\n" > PULSE_RESULT.md' % card["id"])
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertEqual(run["commits"], 0)
        brief = (self.home / "work" / ("%s.brief.md" % job["id"])).read_text(encoding="utf-8")
        self.assertIn("## Task (ballot, Tier 0)", brief)
        self.assertIn("  > The body says why.", brief, "a ballot job's brief quotes the card body for hands without a store")
        votes = self.con.execute("SELECT agent, on_post, vote FROM posts WHERE kind = 'vote'").fetchall()
        self.assertEqual([(v["agent"], v["on_post"], v["vote"]) for v in votes], [("aurora", card["id"], "up")])
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (job["id"],)).fetchone()["status"], "done")

    # -- WU-019: throughput - parallel jobs per project, rescued timeouts, hands wanted ---------------

    def test_parallel_per_project_leases_and_plans_up_to_the_limit(self):
        for name in ("aurora", "cinder", "delta", "echo"):
            pulse.join_agent(self.con, name, "r", "m")
        pulse.finish_job(self.con, self.con.execute("SELECT id FROM jobs").fetchone()["id"], "human", "cancelled")
        cards = []
        for i in range(3):
            card = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed-%d" % i, "cache table %d" % i, "one variable",
                                     agent="aurora", date="2026-09-09T09:0%d:00Z" % i)
            for voter in ("cinder", "delta"):
                pulse.create_post(self.con, "vote", "proj", "evolution", "speed-%d" % i, "up", agent=voter, on=card["id"], vote="up",
                                  date="2026-09-09T10:00:00Z")
            cards.append(card)
        # the default keeps today's rule: one job per project waiting or running
        self.assertEqual(len(pulse.plan_jobs(self.con, "proj", now=NOW)), 1)
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now=NOW), [])
        first = pulse.lease_job(self.con, "aurora", now=NOW)
        self.assertIsNotNone(first)
        self.assertIsNone(pulse.lease_job(self.con, "cinder", now=NOW), "one leased job per project at the default")
        # raised to 2: the planner tops the project up to two waiting jobs and a second lease goes through
        pulse.set_setting(self.con, "parallel_per_project", "2", "human")
        planned = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual(len(planned), 1, "one more job, up to the limit")
        self.assertEqual(pulse.plan_jobs(self.con, "proj", now=NOW), [], "two are waiting or running: the limit")
        second = pulse.lease_job(self.con, "cinder", now=NOW)
        self.assertIsNotNone(second, "a second job of the same project leases when parallel_per_project is 2")
        self.assertNotEqual(first["id"], second["id"])
        self.assertIsNone(pulse.lease_job(self.con, "delta", now=NOW), "the third waits: two leased already")
        self.assertEqual(pulse.parallel_per_project(self.con), 2)
        with self.assertRaises(pulse.PulseError):
            pulse.set_setting(self.con, "parallel_per_project", "0", "human")
        self.con.execute("UPDATE settings SET value = '0' WHERE name = 'parallel_per_project'")  # an older tool's row
        self.assertEqual(pulse.parallel_per_project(self.con), 1, "never below one")
        code, out, err = run_cli("status")
        self.assertEqual(code, 0, err)
        self.assertIn("parallel_per_project=0", out)

    def test_a_timed_out_worker_keeps_its_verified_commits(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.set_setting(self.con, "jobs_per_day", "0", "human")  # four runs in one test day
        committed = self.worker_script('echo kept >> README.md\ngit commit -qam "kept before the cap"')
        original = pulse.LocalExecutor.run_worker

        def run_then_time_out(ex, cmd, env, minutes):
            original(ex, cmd, env, minutes)
            return {"timeout": True, "exit_code": None, "stdout": "", "stderr": ""}

        with unittest.mock.patch.object(pulse.LocalExecutor, "run_worker", run_then_time_out):
            run, _ = pulse.work_once(self.con, "aurora", worker_cmd=committed, now=NOW)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertEqual(run["commits"], 1)
        self.assertIn("hit the 10-minute cap before writing PULSE_RESULT.md", run["summary"])
        self.assertIn("may be partial", run["summary"])
        self.assertTrue(run["branch"].startswith("pulse/aurora/"))
        self.assertEqual(git(self.repo, "rev-list", "--count", "main..%s" % run["branch"]).stdout.strip(), "1")
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()["status"], "done")
        # a result file written before the cap keeps its summary, marked as a run that hit the cap
        pulse.add_job(self.con, "proj", "task", "again", created_by="test")
        reported = self.worker_script('echo more >> README.md\ngit commit -qam more\nprintf "SUMMARY: added more\\n" > PULSE_RESULT.md',
                                      "reported.sh")
        with unittest.mock.patch.object(pulse.LocalExecutor, "run_worker", run_then_time_out):
            run, _ = pulse.work_once(self.con, "aurora", worker_cmd=reported, now=NOW)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertIn("hit the 10-minute cap after committing: added more", run["summary"])
        # red commits are dropped exactly as they would be for a run that finished
        self.con.execute("UPDATE projects SET verify_cmd = 'grep -q never README.md' WHERE slug = 'proj'")
        pulse.add_job(self.con, "proj", "task", "red", created_by="test")
        with unittest.mock.patch.object(pulse.LocalExecutor, "run_worker", run_then_time_out):
            run, _ = pulse.work_once(self.con, "aurora", worker_cmd=committed, now=NOW)
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertIn("verify exit", run["summary"])
        # nothing committed: a timeout, as before
        self.con.execute("UPDATE projects SET verify_cmd = 'true' WHERE slug = 'proj'")
        pulse.add_job(self.con, "proj", "task", "empty", created_by="test")
        with unittest.mock.patch.object(pulse.LocalExecutor, "run_worker", run_then_time_out):
            run, _ = pulse.work_once(self.con, "aurora", worker_cmd=self.worker_script("true", "nothing.sh"), now=NOW)
        self.assertEqual(run["status"], "timeout", run["summary"])

    def test_hands_wanted_is_one_finding_a_day_and_status_shows_capacity(self):
        self.con.execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'")
        brain = pulse.brain_project(self.con)
        self.assertEqual(pulse.capacity_status(self.con, NOW), "", "nothing recorded yet")
        pulse.capacity_note(self.con, 3, 2, 2, "2026-09-10T11:15:00Z", NOW)
        line = pulse.capacity_status(self.con, NOW)
        self.assertIn("3 job(s) queued, 2 slot(s), 2 in flight", line)
        self.assertIn("every slot busy for 45 min", line)
        self.assertIn("pulse pair", line)
        code, out, err = run_cli("status")
        self.assertEqual(code, 0, err)
        self.assertIn("capacity   3 job(s) queued", out)
        post = pulse.hands_wanted(self.con, brain, 3, 2, 45, NOW)
        self.assertIsNotNone(post)
        self.assertEqual((post["kind"], post["area"], post["thread"]), ("finding", "runner", "hands-wanted"))
        self.assertIn("hands wanted: 3 job(s) waited 45 min with all 2 slot(s) busy", post["summary"])
        self.assertIn("not a request to act", post["body"])
        self.assertIsNone(pulse.hands_wanted(self.con, brain, 4, 2, 60, NOW), "one finding a day")
        self.assertIsNone(pulse.hands_wanted(self.con, None, 4, 2, 60, NOW), "no brain, no post")
        pulse.capacity_note(self.con, 0, 2, 1, "", NOW)
        self.assertEqual(pulse.capacity_status(self.con, NOW), "", "nothing waits: no line")

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

    # -- WU-009: the loop's leaks -----------------------------------------

    def test_a_login_failure_on_this_machine_is_charged_like_any_runner_error(self):
        # no executor row to drain here (WU-014): the daemon log shows the reason, the job keeps its attempt count
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        unauth = self.worker_script('echo "Error: re-authenticate with copilot login" >&2\nexit 1', "unauth.sh")
        job_id = self.con.execute("SELECT id FROM jobs").fetchone()["id"]
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=unauth, now=NOW)
        self.assertEqual(run["status"], "error")
        self.assertIn("rejected at launch", run["summary"])
        self.assertNotIn("the login on", run["summary"])
        job = self.con.execute("SELECT status, attempts FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 1))

    def test_runner_errors_requeue_the_job_and_do_not_count_against_the_cap(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        rejected = self.worker_script('echo "error: option --max-ai-credits argument 5 is invalid" >&2\nexit 1', "rejected.sh")
        job_id = self.con.execute("SELECT id FROM jobs").fetchone()["id"]
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=rejected, now=NOW)
        self.assertEqual(run["status"], "error")
        self.assertIn("rejected at launch", run["summary"])
        self.assertIn("max-ai-credits", run["summary"])
        self.assertEqual(pulse.runs_today(self.con, NOW), 0, "a runner error is not a worker run")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"], job["lease_agent"]), ("queued", 1, ""))
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_failed"], 0, "not the agent's fault")
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM posts WHERE kind = 'result'").fetchone()[0], 0, "no board noise per retry")
        # a missing worker command is a runner error too
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=str(self.tmp / "no-such-worker"), now=NOW)
        self.assertEqual(run["status"], "error")
        self.assertIn("not found", run["summary"])
        # the third error exhausts the attempts: the job fails and one result says so
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=rejected, now=NOW)
        self.assertEqual(run["status"], "error")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("failed", 3))
        self.assertIn("after 3 runner errors", job["result"])
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM posts WHERE kind = 'result'").fetchone()[0], 1)
        self.assertEqual(pulse.runs_today(self.con, NOW), 0)
        # a worker that ran and then failed still counts: it spent the credits
        pulse.add_job(self.con, "proj", "task", "real failure", created_by="test")
        slow_fail = self.worker_script('printf "SUMMARY: tried\\n" > PULSE_RESULT.md\nexit 1', "slow-fail.sh")
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=slow_fail, now=NOW)
        self.assertEqual(run["status"], "failed")
        self.assertEqual(pulse.runs_today(self.con, NOW), 1)
        # the daemon waits after a runner error instead of spinning through the queue
        pulse.add_job(self.con, "proj", "task", "again", created_by="test")
        out = io.StringIO()
        with unittest.mock.patch.object(pulse.time, "sleep") as sleep:
            pulse.daemon(self.con, interval=7, worker_cmd=rejected, max_loops=2, out=out)
        sleep.assert_called_with(7)
        self.assertIn("error", out.getvalue())

    def test_brief_tells_the_worker_to_vote_through_the_same_store(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.join_agent(self.con, "cinder", "r", "m")
        sug = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the table", "one variable",
                                agent="cinder", date="2026-09-09T09:00:00Z")
        # the worker does what the brief says: ballot, then one vote with a reason, through $PULSE_CLI and $PULSE_HOME
        worker = self.worker_script(
            'grep -q "ballot --project proj" "$PULSE_BRIEF"\n'
            'python3 "$PULSE_CLI" ballot --project "$PULSE_PROJECT" | grep -q evolution/speed\n'
            'python3 "$PULSE_CLI" vote %s up --summary "one variable, measurable" > /dev/null\n'
            'printf "SUMMARY: voted\\n" > PULSE_RESULT.md' % sug["id"])
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", Path(run["log"]).read_text())
        votes = self.con.execute("SELECT agent, vote, on_post FROM posts WHERE kind = 'vote'").fetchall()
        self.assertEqual([(v["agent"], v["vote"], v["on_post"]) for v in votes], [("aurora", "up", sug["id"])])
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM posts WHERE kind = 'vote'").fetchone()[0], 1, "one store, no stray home")

    def test_daemon_keeps_a_worker_pool_of_quorum_plus_one_and_rotates_it(self):
        first = pulse.pick_worker(self.con)
        names = {r["name"] for r in self.con.execute("SELECT name FROM agents WHERE kind = 'worker' AND status = 'active'")}
        self.assertEqual(len(names), 3, "quorum 2 needs two distinct voters besides the card's own author")
        self.assertIn(first, names)
        job = pulse.lease_job(self.con, first, now=NOW)
        pulse.finish_job(self.con, job["id"], first, "done", generation=job["generation"])
        second = pulse.pick_worker(self.con)
        self.assertNotEqual(first, second, "round-robin: the worker that leased least recently goes next")
        pulse.add_job(self.con, "proj", "task", "two", created_by="test")
        job = pulse.lease_job(self.con, second, now=NOW)
        pulse.finish_job(self.con, job["id"], second, "done", generation=job["generation"])
        third = pulse.pick_worker(self.con)
        self.assertEqual(len({first, second, third}), 3, "every worker in the pool gets a turn before the first repeats")
        pulse.add_job(self.con, "proj", "task", "three", created_by="test")
        job = pulse.lease_job(self.con, third, now=NOW)
        pulse.finish_job(self.con, job["id"], third, "done", generation=job["generation"])
        self.assertEqual(pulse.pick_worker(self.con), first)
        self.assertEqual(len({r["name"] for r in self.con.execute("SELECT name FROM agents WHERE kind = 'worker'")}), 3, "no unbounded growth")
        self.assertEqual(pulse.pick_worker(self.con, agent=first), first, "an explicit agent is honored")

    def test_the_lease_rotates_across_projects_so_a_busy_brain_never_starves_the_others(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.set_setting(self.con, "jobs_per_day", "0", "human")
        other = make_repo(self.tmp, "other")
        pulse.add_project(self.con, other, verify_cmd="true")
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")
        for n in range(3):
            pulse.add_job(self.con, "proj", "task", "brain %d" % n, priority=13, created_by="test")
        pulse.add_job(self.con, "other", "task", "small fix", priority=0, created_by="test")
        worker = self.worker_script('printf "SUMMARY: looked\n" > PULSE_RESULT.md')
        order = []
        for minute in range(4):
            now = "2026-09-10T12:%02d:00Z" % minute
            run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=now)
            self.assertIsNotNone(run, reason)
            order.append(run["project"])
        self.assertEqual(order, ["proj", "other", "proj", "proj"],
                         "no run anywhere: priority decides; then the project that never ran goes before the one that just did")
        # within a project, priority still decides
        pulse.add_job(self.con, "proj", "task", "low", priority=1, created_by="test")
        pulse.add_job(self.con, "proj", "task", "high", priority=9, created_by="test")
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(self.con.execute("SELECT title FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()["title"], "high")

    def test_a_stale_lease_never_lands_and_never_finishes_the_job(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.join_agent(self.con, "cinder", "r", "m", kind="worker")
        job = pulse.lease_job(self.con, "aurora", now=NOW)
        with self.assertRaises(pulse.PulseError):
            pulse.finish_job(self.con, job["id"], "aurora", "done", generation=job["generation"] + 1)
        self.assertTrue(pulse.renew_lease(self.con, job["id"], "aurora", job["generation"], 5, now=NOW))
        self.assertFalse(pulse.renew_lease(self.con, job["id"], "aurora", job["generation"] + 1, 5, now=NOW), "a stale holder cannot renew")
        pulse.finish_job(self.con, job["id"], "aurora", "queued", generation=job["generation"])
        # A worker whose lease expired mid-run: another runner requeues and retakes the job while it works.
        store = str(self.home / "pulse.sqlite")
        worker = self.worker_script(
            'echo late >> README.md\n'
            'git commit -qam "late change"\n'
            'python3 -c "import sqlite3; c = sqlite3.connect(%r); '
            'c.execute(\\"UPDATE jobs SET generation = generation + 1, lease_agent = \'cinder\' WHERE id = ?\\", (%r,)); c.commit()"\n'
            'printf "SUMMARY: too late\\n" > PULSE_RESULT.md' % (store, job["id"]))
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "stale", Path(run["log"]).read_text())
        self.assertEqual(run["branch"], "")
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "nothing landed")
        job_now = self.con.execute("SELECT status, lease_agent FROM jobs WHERE id = ?", (job["id"],)).fetchone()
        self.assertEqual((job_now["status"], job_now["lease_agent"]), ("leased", "cinder"), "the new holder's job is untouched")
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM posts WHERE kind = 'result'").fetchone()[0], 0)
        self.assertEqual(pulse.runs_today(self.con, NOW), 1, "the worker did run and spend; the cap counts it")

    def test_ballot_queue_and_jobs_print_whole_ids_a_worker_can_paste(self):
        long_name = "claude-fable-5-1-release-reviewer-ab"  # a legacy 36-character name; ids grow past 44
        pulse.join_agent(self.con, long_name, "r", "m")
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        post = pulse.create_post(self.con, "suggestion", "proj", "governance", "long-ids", "a card", "", agent=long_name)
        self.assertGreater(len(post["id"]), 44)
        job = pulse.lease_job(self.con, "aurora", now=NOW)
        pulse.finish_job(self.con, job["id"], "aurora", "queued", generation=job["generation"])
        for args in (("ballot", "--project", "proj", "--agent", "aurora"), ("queue", "--project", "proj"),
                     ("board", "--project", "proj", "--area", "governance")):
            code, out, err = run_cli(*args)
            self.assertEqual(code, 0, err)
            self.assertIn(post["id"], out, "%s clipped the id" % args[0])
            self.assertNotIn(post["id"][:43] + "~", out)
        code, out, err = run_cli("jobs", "--project", "proj")
        self.assertEqual(code, 0, err)
        self.assertIn(job["id"], out)

    # -- WU-010: keep-alive runner, no daily cap, self-feeding suggestions --

    def test_jobs_per_day_zero_means_no_daily_cap(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.set_setting(self.con, "jobs_per_day", "0", "human")
        self.assertEqual(pulse.daily_cap(self.con), 0)
        worker = self.worker_script('printf "SUMMARY: looked around\\n" > PULSE_RESULT.md')
        for i in range(3):
            pulse.add_job(self.con, "proj", "task", "job %d" % i, created_by="test")
        for i in range(3):
            run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW[:10] + "T13:0%d:00Z" % i)
            self.assertIsNotNone(run, reason)
            self.assertEqual(run["status"], "done")
        self.assertEqual(pulse.runs_today(self.con, NOW), 3)
        code, out, err = run_cli("status")
        self.assertEqual(code, 0, err)
        self.assertIn("no daily cap", out)
        # the other spellings of "none"; an empty value is a typo the setter refuses (ADR-025), though a
        # legacy empty row is still read as no cap
        for word in ("none", "off", "unlimited"):
            pulse.set_setting(self.con, "jobs_per_day", word, "human")
            self.assertEqual(pulse.daily_cap(self.con), 0, word)
        with self.assertRaises(pulse.PulseError):
            pulse.set_setting(self.con, "jobs_per_day", "", "human")
        self.con.execute("UPDATE settings SET value = '' WHERE name = 'jobs_per_day'")
        self.assertEqual(pulse.daily_cap(self.con), 0)
        pulse.set_setting(self.con, "jobs_per_day", "2", "human")
        self.assertEqual(pulse.daily_cap(self.con), 2)
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW[:10] + "T14:00:00Z")
        self.assertIsNone(run)
        self.assertIn("daily cap", reason)

    def test_worker_suggestion_lines_become_suggestion_posts_the_next_ballot_shows(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.join_agent(self.con, "cinder", "r", "m", kind="worker")
        worker = self.worker_script(
            'printf "SUMMARY: onboarded\\n'
            'SUGGESTION: evolution/speed: cache the tally table so board renders in half the time\\n'
            'SUGGESTION: drop the README banner\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done")
        sugs = self.con.execute("SELECT * FROM posts WHERE kind = 'suggestion' ORDER BY id").fetchall()
        self.assertEqual(len(sugs), 2)
        by_thread = {s["thread"]: s for s in sugs}
        named = by_thread["speed"]
        self.assertEqual(named["area"], "evolution")
        self.assertIn("cache the tally table", named["summary"])
        self.assertEqual(named["agent"], "aurora", "provenance is the worker that saw it")
        self.assertIn(run["job_id"], named["body"])
        default = [s for s in sugs if s["thread"] == run["job_id"].lower()]
        self.assertEqual(len(default), 1, "no area/thread given: filed under evolution/<job id>, which the job's result does not close")
        self.assertEqual(default[0]["area"], "evolution")
        self.assertIn("drop the README banner", default[0]["summary"])
        # the next worker's ballot lists them, so the loop feeds itself
        code, out, err = run_cli("ballot", "--project", "proj", "--agent", "cinder")
        self.assertEqual(code, 0, err)
        for s in sugs:
            self.assertIn(s["id"], out)
        code, out, err = run_cli("ballot", "--project", "proj", "--agent", "aurora")
        self.assertEqual(code, 0, err)
        for s in sugs:
            self.assertNotIn(s["id"], out, "no self-votes: the author's ballot does not list its own cards")

    def test_parse_suggestion_line_splits_area_and_thread_only_when_well_formed(self):
        self.assertEqual(pulse.parse_suggestion_line("evolution/speed: cache it"), ("evolution", "speed", "cache it"))
        self.assertEqual(pulse.parse_suggestion_line("jobs/wu-010 - shorter briefs"), ("jobs", "wu-010", "shorter briefs"))
        self.assertEqual(pulse.parse_suggestion_line("use https://x/y: not an area"), ("", "", "use https://x/y: not an area"))
        self.assertEqual(pulse.parse_suggestion_line("Evolution/Speed: caps are not areas"), ("", "", "Evolution/Speed: caps are not areas"))
        # a worker that copies the brief's placeholder literally gets the defaults, not a thread called "thread"
        self.assertEqual(pulse.parse_suggestion_line("evolution/thread: make C2 explicit"), ("", "", "make C2 explicit"))
        self.assertEqual(pulse.parse_suggestion_line("area/thread: make C2 explicit"), ("", "", "make C2 explicit"))

    def test_a_clone_commit_is_the_agents_signed_by_the_runs_key_whatever_the_machine_signs_with(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script(
            'printf "%s %s %s" "$(git config --get commit.gpgsign)" "$(git config --get gpg.ssh.program)" "$(git config --get core.hooksPath)" > sign.txt\n'
            'echo "changed by $PULSE_AGENT" >> README.md\n'
            'git commit -qam "plain commit, no -c flags"\n'
            'printf "SUMMARY: signing=%s\\n" "$(cat sign.txt)" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        # every knob the signature depends on is pinned in the clone: a machine-wide signer or hooks dir never takes over
        self.assertIn("signing=true ssh-keygen %s" % (self.home / "work" / run["job_id"] / ".git" / "hooks"), run["summary"])
        branch = run["branch"]
        self.assertTrue(branch, "the plain commit landed")
        who = git(self.repo, "log", "-1", "--format=%an <%ae>", branch).stdout.strip()
        self.assertEqual(who, "aurora <aurora@pulse.local>", "the commit is the agent's, not the human's")
        self.assertEqual(self.signature_status(branch, "aurora", run["signing_pubkey"]), "G")

    # -- ADR-022: the hub lands only what this run's key signed, as this agent, for this job ------------------

    def refused_run(self, worker_body):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script(worker_body + '\nprintf "SUMMARY: tried\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertEqual(run["branch"], "")
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "nothing landed")
        self.assertEqual(git(self.repo, "for-each-ref", "refs/pulse/").stdout.strip(), "", "no holding ref left behind")
        self.assertIn("nothing landed", run["summary"])
        return run

    def test_work_refuses_a_commit_by_another_author(self):
        run = self.refused_run('echo x >> README.md\ngit -c user.name=w -c user.email=w@x commit -qam "as someone else"')
        self.assertIn("is authored by w <w@x>, not aurora", run["summary"])

    def test_work_refuses_an_unsigned_commit(self):
        run = self.refused_run('echo x >> README.md\ngit commit --no-gpg-sign -qam "unsigned"')
        self.assertIn("is not signed", run["summary"])

    def test_work_refuses_a_commit_signed_by_another_key(self):
        run = self.refused_run('ssh-keygen -q -t ed25519 -N "" -f "$PULSE_CLONE/../other.key"\necho x >> README.md\n'
                               'git -c user.signingkey="$PULSE_CLONE/../other.key" commit -qam "other key"\nrm -f "$PULSE_CLONE/../other.key"*')
        self.assertIn("is signed by a key that is not this run's", run["summary"])

    def test_work_refuses_a_commit_made_without_the_provenance_hook(self):
        # `--no-verify` skips pre-commit and commit-msg only; prepare-commit-msg still stamps the trailers, so the
        # one way around the hook is to point core.hooksPath elsewhere - and that commit lands nothing.
        run = self.refused_run('echo y >> README.md\ngit -c core.hooksPath=/nonexistent commit -qam "no trailers"')
        self.assertIn("lacks its Pulse-Agent trailer", run["summary"])
        self.assertIn("lacks its Pulse-Job trailer", run["summary"])
        pulse.add_job(self.con, "proj", "task", "a second run", tier=0)
        worker = self.worker_script('echo x >> README.md\ngit commit --no-verify -qam "still stamped"\nprintf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertIn("Pulse-Job: %s" % run["job_id"], git(self.repo, "log", "-1", "--format=%(trailers)", run["branch"]).stdout)

    def test_verify_and_signers_read_a_landed_commit_back_to_its_run(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script('echo x >> README.md\ngit commit -qam "signed work"\nprintf "SUMMARY: x\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "done", run["summary"])
        code, out, err = run_cli("verify", run["branch"], "--project", "proj")
        self.assertEqual(code, 0, out + err)
        self.assertIn("verdict   verified: signed by the key of run %s, authored by aurora, job %s on local" % (run["id"], run["job_id"]), out)
        self.assertIn("Pulse-Executor: local", out)
        code, out, err = run_cli("verify", "main", "--project", "proj")
        self.assertEqual(code, 1, "the human's own commit is not a worker's")
        self.assertIn("no Pulse-Job trailer", out)
        code, out, err = run_cli("signers", "--project", "proj")
        self.assertEqual(code, 0, err)
        self.assertEqual(out.strip(), 'aurora@pulse.local namespaces="git" %s proj/%s' % (run["signing_pubkey"], run["id"]))
        # the printed file is what git needs to show the signature as good on any checkout
        signers = self.tmp / "signers"
        signers.write_text(out, encoding="utf-8")
        status = subprocess.run(["git", "-C", str(self.repo), "-c", "gpg.ssh.program=ssh-keygen", "-c", "gpg.ssh.allowedSignersFile=%s" % signers,
                                 "log", "-1", "--format=%G?", run["branch"]], capture_output=True, text=True).stdout.strip()
        self.assertEqual(status, "G")
        self.assertNotIn("PRIVATE", json.dumps([dict(r) for r in self.con.execute("SELECT * FROM runs")]), "no private key in the store")

    def test_a_run_is_the_workers_session_and_its_posts_carry_the_model(self):
        pulse.join_agent(self.con, "aurora", "copilot", "", kind="worker")
        pulse.set_setting(self.con, "worker_model", "tiny-model", "human")
        worker = self.worker_script(
            'python3 "$PULSE_CLI" post finding --area evolution --thread seen --summary "saw it" >/dev/null\n'
            'printf "SUMMARY: looked\nSUGGESTION: evolution/thread: card\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done")
        row = pulse.agent_row(self.con, "aurora")
        self.assertIn(run["id"], json.loads(row["sessions"]), "the run id is the worker's session")
        self.assertEqual(row["kind"], "worker")
        posts = self.con.execute("SELECT kind, session, thread FROM posts WHERE agent = 'aurora' ORDER BY id").fetchall()
        kinds = {p["kind"] for p in posts}
        self.assertEqual(kinds, {"finding", "result", "suggestion"})
        for p in posts:
            self.assertEqual(p["session"], run["id"], "%s post carries the run as its session" % p["kind"])
        sug = [p for p in posts if p["kind"] == "suggestion"][0]
        self.assertEqual(sug["thread"], run["job_id"].lower(), "placeholder area/thread falls back to the default")

    def test_votes_from_the_authors_session_are_self_votes(self):
        # a sub-agent spawned inside the author's session shares its session id and must not lift the card
        pulse.join_agent(self.con, "aurora", "r", "m", session="sess-1")
        pulse.join_agent(self.con, "cinder", "r", "m", session="sess-1")
        pulse.join_agent(self.con, "dusk", "r", "m", session="sess-2")
        sug = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the table", "one variable", agent="aurora")
        self.assertEqual(sug["session"], "sess-1")
        with self.assertRaises(pulse.PulseError) as ctx:
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: same session", agent="cinder", on=sug["id"], vote="up")
        self.assertIn("session", str(ctx.exception))
        code, out, err = run_cli("ballot", "--project", "proj", "--agent", "cinder")
        self.assertEqual(code, 0, err)
        self.assertNotIn(sug["id"], out, "the ballot hides cards from the voter's own session")
        code, out, err = run_cli("ballot", "--project", "proj", "--agent", "dusk")
        self.assertEqual(code, 0, err)
        self.assertIn(sug["id"], out)
        pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: fewer retries", agent="dusk", on=sug["id"], vote="up")
        t, _ = pulse.tally_for(self.con, "proj")
        rec = [s for s in t.suggestion_records() if s["id"] == sug["id"]][0]
        self.assertEqual((rec["voters"], rec["up"]), (1, 1))
        # the human is exempt, as for author self-votes
        pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: owner", agent=pulse.HUMAN_AGENT, on=sug["id"], vote="up",
                          session="sess-1")

    def test_plan_jobs_skips_worker_cards_without_a_resolving_signal(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        for name in ("cinder", "dusk"):
            pulse.join_agent(self.con, name, "r", "m")
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")
        guess = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "drop the README banner",
                                  "One-variable improvement seen by the worker.", agent="aurora", date="2026-09-09T09:00:00Z")
        for voter in ("cinder", "dusk"):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: because", "", agent=voter,
                              on=guess["id"], vote="up", date="2026-09-09T10:00:00Z")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["kind"] for j in jobs], ["reflect"], "an evidence-free worker card is never planned")
        code, out, err = run_cli("ballot", "--project", "proj", "--agent", "cinder")
        self.assertEqual(code, 0, err)
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")
        # a Signal that resolves - a path in the project tree, a post id, or a check name - is planned
        for i, signal in enumerate(("Signal: README.md", "Signal: %s" % guess["id"], "Signal: check:unit")):
            card = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "trim the README %d. %s" % (i, signal),
                                     "One variable.\n%s" % signal, agent="aurora", date="2026-09-09T11:0%d:00Z" % i)
            for voter in ("cinder", "dusk"):
                pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: because", "", agent=voter,
                                  on=card["id"], vote="up", date="2026-09-09T12:00:00Z")
            jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
            self.assertEqual([j["source"] for j in jobs], ["suggestion:%s" % card["id"]], signal)
            for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
                pulse.finish_job(self.con, j["id"], "human", "cancelled")
        # a path outside the tree does not resolve; a session agent's card is not filtered at all
        bad = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "escape. Signal: ../etc/passwd", "",
                                agent="aurora", date="2026-09-09T13:00:00Z")
        human_card = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "session agent card, no signal", "",
                                       agent="cinder", date="2026-09-09T13:01:00Z")
        for voter, on in (("cinder", bad), ("dusk", bad), ("aurora", human_card), ("dusk", human_card)):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: because", "", agent=voter,
                              on=on["id"], vote="up", date="2026-09-09T14:00:00Z")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["source"] for j in jobs], ["suggestion:%s" % human_card["id"]])

    def test_plan_jobs_builds_only_suggestions_voted_up_on_balance(self):
        for name in ("aurora", "cinder", "dusk"):
            pulse.join_agent(self.con, name, "r", "m")
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")
        down = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "delete the tests", "one variable",
                                 agent="aurora", date="2026-09-09T09:00:00Z")
        up = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "cache the tally", "one variable",
                               agent="aurora", date="2026-09-09T09:01:00Z")
        for voter, on, value in (("cinder", down, "down"), ("dusk", down, "down"), ("cinder", up, "up"), ("dusk", up, "up")):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "%s: because" % value, "", agent=voter,
                              on=on["id"], vote=value, date="2026-09-09T10:00:00Z")
        jobs = pulse.plan_jobs(self.con, "proj", now=NOW)
        improve = [j for j in jobs if j["kind"] == "improve"]
        self.assertEqual(len(improve), 1, [j["title"] for j in jobs])
        self.assertEqual(improve[0]["source"], "suggestion:%s" % up["id"])

    def test_a_suggestion_is_built_once_then_leaves_the_queue_and_the_ballot(self):
        for name in ("aurora", "cinder", "dusk"):
            pulse.join_agent(self.con, name, "r", "m", kind="worker" if name == "aurora" else "session")
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")
        card = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "shorter README", "one variable",
                                 agent="cinder", date="2026-09-09T09:00:00Z")
        for voter in ("aurora", "dusk"):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: because", "", agent=voter,
                              on=card["id"], vote="up", date="2026-09-09T10:00:00Z")
        first = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["source"] for j in first], ["suggestion:%s" % card["id"]])
        worker = self.worker_script(
            'printf "trimmed\n" >> README.md && git add README.md && git commit -qm trim\n'
            'printf "SUMMARY: trimmed the README\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done")
        # the card is built: the queue shows it, the ballot drops it, the planner never rebuilds it
        t, _ = pulse.tally_for(self.con, "proj", None, NOW)
        rec = [r for r in t.suggestion_records() if r["id"] == card["id"]][0]
        self.assertEqual(rec["status"], "built")
        code, out, err = run_cli("board", "--project", "proj", "--agent", "dusk")
        self.assertEqual(code, 0, err)
        self.assertRegex(out, r"built\s+evolution/speed")
        self.assertNotIn(card["id"], run_cli("ballot", "--project", "proj", "--agent", "dusk")[1])
        again = pulse.plan_jobs(self.con, "proj", now=NOW)
        self.assertEqual([j["source"] for j in again if j["kind"] == "improve"], [],
                         "one improve job per suggestion, ever - the daemon does not rebuild a built card")
        # a failed attempt is "tried" and also leaves the queue
        other = pulse.create_post(self.con, "suggestion", "proj", "evolution", "speed", "drop the banner", "one variable",
                                  agent="cinder", date="2026-09-09T09:30:00Z")
        for voter in ("aurora", "dusk"):
            pulse.create_post(self.con, "vote", "proj", "evolution", "speed", "up: sure", "", agent=voter,
                              on=other["id"], vote="up", date="2026-09-09T10:30:00Z")
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")  # the fallback reflect job the planner added
        job = pulse.plan_jobs(self.con, "proj", now=NOW)[0]
        self.assertEqual(job["source"], "suggestion:%s" % other["id"])
        pulse.finish_job(self.con, job["id"], "human", "failed")
        t, _ = pulse.tally_for(self.con, "proj", None, NOW)
        self.assertEqual({r["id"]: r["status"] for r in t.suggestion_records()}[other["id"]], "tried")
        self.assertEqual([j for j in pulse.plan_jobs(self.con, "proj", now=NOW) if j["kind"] == "improve"], [])

    def test_daemon_writes_a_heartbeat_survives_a_bad_loop_and_restarts_on_code_change(self):
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script('printf "SUMMARY: looked around\\n" > PULSE_RESULT.md')
        source = self.tmp / "fake-pulse.py"
        source.write_text("# v1\n")
        out = io.StringIO()
        with unittest.mock.patch.object(pulse, "plan_jobs", side_effect=[RuntimeError("boom"), [], []]), \
                unittest.mock.patch.object(pulse.time, "sleep"):
            result = pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=2, out=out, source_path=str(source))
        self.assertEqual(result, "done")
        text = out.getvalue()
        self.assertIn("daemon error: RuntimeError: boom", text)
        self.assertIn("done", text, "the loop after the error ran the job")
        self.assertIn("exited", pulse.daemon_status(self.con))
        # a live heartbeat while looping
        pulse.daemon_heartbeat(self.con, 5, 300)
        status = pulse.daemon_status(self.con, now=NOW)
        self.assertIn("pid %d" % os.getpid(), status)
        self.assertIn("5 loop(s)", status)
        self.assertTrue(status.startswith("alive") or status.startswith("silent"))
        code, cli_out, err = run_cli("status")
        self.assertEqual(code, 0, err)
        self.assertIn("daemon", cli_out)
        # the code on disk changed: the daemon returns "restart" so the process re-executes itself
        out = io.StringIO()
        pulse.add_job(self.con, "proj", "task", "again", created_by="test")
        def bump(*a, **k):
            source.write_text("# v2\n")
            os.utime(source, (1, 1))
            return []
        with unittest.mock.patch.object(pulse, "plan_jobs", side_effect=bump):
            result = pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=5, out=out, source_path=str(source),
                                  restart_on_change=True)
        self.assertEqual(result, "restart")
        self.assertIn("changed on disk", out.getvalue())
        self.assertLessEqual(out.getvalue().count("done "), 1, "it stopped at the first loop after the change")
        # a save that does not compile (an edit in progress) never takes the loop down: it keeps running the old code,
        # says so once, and restarts only when the file compiles again
        out = io.StringIO()
        pulse.add_job(self.con, "proj", "task", "once more", created_by="test")
        source.write_text("# v3\n")
        os.utime(source, (2, 2))
        saves = iter([("def broken(:\n", (3, 3)), ("def broken(:\n", (3, 3)), ("# v4\n", (4, 4))])
        def save_next(*a, **k):
            text, times = next(saves, (None, None))
            if text is not None:
                source.write_text(text)
                os.utime(source, times)
            return []
        with unittest.mock.patch.object(pulse, "plan_jobs", side_effect=save_next):
            result = pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=6, out=out, source_path=str(source),
                                  restart_on_change=True)
        self.assertEqual(result, "restart")
        self.assertEqual(out.getvalue().count("does not compile"), 1, "one warning per broken save, not one per loop")
        self.assertIn("line 1:", out.getvalue())
        self.assertEqual(out.getvalue().count("changed on disk; restarting"), 1)
        self.assertEqual(pulse.source_compiles(str(pulse.__file__)), (True, ""))

    def test_worker_command_denies_the_remote_tools_and_runs_in_autopilot(self):
        cmd, custom = pulse.worker_command(self.con, "the brief", Path("/tmp/clone"))
        self.assertFalse(custom)
        self.assertEqual(cmd[:3], ["copilot", "-p", "the brief"])
        for needle in ("--allow-all-tools", "--disable-builtin-mcps", "--no-ask-user", "-s", "--autopilot"):
            self.assertIn(needle, cmd)
        self.assertEqual(cmd[cmd.index("--max-ai-credits") + 1], "30")
        denied = [cmd[i + 1] for i, a in enumerate(cmd) if a == "--deny-tool"]
        self.assertEqual(denied, list(pulse.DENIED_TOOLS))
        self.assertTrue(any("git push" in d for d in denied) and any("gh" in d for d in denied))
        self.assertNotIn("--model", cmd, "empty worker_model means the runner's default")
        self.assertNotIn("--allow-all-paths", cmd, "no broad path grant: only the store, the CLI's folder, and scratch")
        added = [cmd[i + 1] for i, a in enumerate(cmd) if a == "--add-dir"]
        self.assertIn(str(self.home.resolve()), added, "the store the worker votes through")
        self.assertIn(str(Path(pulse.__file__).resolve().parent), added, "the pulse.py the brief tells it to call")
        self.assertIn(tempfile.gettempdir(), added)
        pulse.set_setting(self.con, "worker_model", "claude-haiku-4.5", "human")
        cmd, _ = pulse.worker_command(self.con, "the brief", Path("/tmp/clone"))
        self.assertEqual(cmd[-2:], ["--model", "claude-haiku-4.5"])
        cmd, custom = pulse.worker_command(self.con, "the brief", Path("/tmp/clone"), worker_cmd="sh -c 'echo x'")
        self.assertEqual((cmd, custom), (["sh", "-c", "echo x"], True))

    def test_service_files_keep_the_daemon_alive_and_carry_the_path(self):
        plist = pulse.service_file_text("launchd", 120, "/usr/bin/python3", "/repo/docs/scripts/pulse.py",
                                        "/home/u/.pulse/logs/daemon.log", "/opt/homebrew/bin:/usr/bin", "/home/u/.pulse")
        for needle in ("<string>pulse.daemon</string>", "<key>KeepAlive</key><true/>", "<key>RunAtLoad</key><true/>",
                       "<string>--interval</string>", "<string>120</string>", "/opt/homebrew/bin:/usr/bin",
                       "<key>PULSE_HOME</key><string>/home/u/.pulse</string>", "daemon.log"):
            self.assertIn(needle, plist)
        unit = pulse.service_file_text("systemd", 120, "/usr/bin/python3", "/repo/docs/scripts/pulse.py",
                                       "/home/u/.pulse/logs/daemon.log", "/usr/bin", "/home/u/.pulse")
        for needle in ("Restart=always", "ExecStart=/usr/bin/python3 /repo/docs/scripts/pulse.py daemon --interval 120",
                       "Environment=PATH=/usr/bin", "WantedBy=default.target"):
            self.assertIn(needle, unit)
        kind, path = pulse.service_paths("darwin", home="/home/u")
        self.assertEqual((kind, str(path)), ("launchd", "/home/u/Library/LaunchAgents/pulse.daemon.plist"))
        kind, path = pulse.service_paths("linux", home="/home/u")
        self.assertEqual((kind, str(path)), ("systemd", "/home/u/.config/systemd/user/pulse-daemon.service"))
        self.assertEqual(pulse.service_commands("launchd", path, "install")[1][:2], ["launchctl", "bootstrap"])
        self.assertEqual(pulse.service_commands("systemd", path, "install")[-1][-2:], ["--now", "pulse-daemon"])
        # `service show` renders without touching launchctl or systemctl
        code, out, err = run_cli("service", "show", "--interval", "45")
        self.assertEqual(code, 0, err)
        self.assertIn("<string>45</string>" if sys.platform == "darwin" else "--interval 45", out)

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


class LandingTestCase(StoreFixture):
    """ADR-024: verified work lands on the default branch by itself; nothing that conflicts or fails the check ever does."""

    def work(self, body, agent="aurora"):
        if pulse.agent_row(self.con, agent) is None:
            pulse.join_agent(self.con, agent, "r", "m")
        body += '\nprintf "SUMMARY: %s\\n" > PULSE_RESULT.md' % "one line appended"
        run, reason = pulse.work_once(self.con, agent, worker_cmd=self.worker_script(body, name="%s.sh" % agent), now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertTrue(run["branch"])
        return run

    def project(self):
        return pulse.project_rows(self.con)[0]

    def test_land_merges_a_verified_branch_and_removes_it(self):
        run = self.work('echo "changed by $PULSE_AGENT" >> README.md && git add -A && git commit -qm "worker change"')
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["landed"], results)
        self.assertIn("changed by aurora", (self.repo / "README.md").read_text(encoding="utf-8"))
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "the landed branch is removed")
        self.assertEqual(git(self.repo, "rev-parse", "--abbrev-ref", "HEAD").stdout.strip(), "main")
        self.assertEqual(git(self.repo, "status", "--porcelain").stdout.strip(), "", "the checkout is clean after the fast-forward")
        self.assertEqual(git(self.repo, "worktree", "list").stdout.count("\n"), 1, "the landing worktree is gone")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "done")
        self.assertIn("landed", job["result"])
        post = self.con.execute("SELECT * FROM posts WHERE kind = 'result' AND resolution = 'proceeded'").fetchone()
        self.assertIsNotNone(post, "the landing is a result post")
        self.assertIn("landed on main", post["summary"])
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'land.landed'").fetchone()[0], 1)
        self.assertEqual(pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW), [], "nothing waits any more")

    def test_land_defers_while_the_checkout_is_busy_then_merges(self):
        self.work('echo "changed by $PULSE_AGENT" >> README.md && git add -A && git commit -qm "worker change"')
        (self.repo / "AGENTS.md").write_text("# rules\n\nedited by the human\n", encoding="utf-8")
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["deferred"], results)
        self.assertIn("uncommitted", results[0][1])
        self.assertTrue(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "the branch waits")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "human work on main")
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["landed"], results)
        self.assertIn("changed by aurora", (self.repo / "README.md").read_text(encoding="utf-8"))
        self.assertIn("edited by the human", (self.repo / "AGENTS.md").read_text(encoding="utf-8"))
        self.assertIn("Land pulse/aurora/", git(self.repo, "log", "-1", "--format=%s").stdout, "a merge commit when main moved")
        self.assertEqual(git(self.repo, "status", "--porcelain").stdout.strip(), "")

    def test_land_requeues_a_conflicting_job_and_keeps_a_patch(self):
        run = self.work('echo "changed by $PULSE_AGENT" >> README.md && git add -A && git commit -qm "worker change"')
        (self.repo / "README.md").write_text("rewritten on main\n", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "conflicting human change")
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["conflict"], results)
        self.assertEqual((self.repo / "README.md").read_text(encoding="utf-8"), "rewritten on main\n", "main is untouched")
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "the conflicting branch is removed")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual((job["status"], job["attempts"], job["branch"]), ("queued", 0, ""), "the job runs again on the current main")
        self.assertIn("conflicts with main", job["result"])
        patches = list((self.home / "patches" / "proj").glob("*.patch"))
        self.assertEqual(len(patches), 1, "the earlier attempt is kept as a patch")
        self.assertIn("changed by aurora", patches[0].read_text(encoding="utf-8"))
        self.assertEqual(git(self.repo, "status", "--porcelain").stdout.strip(), "")
        self.assertEqual(git(self.repo, "worktree", "list").stdout.count("\n"), 1)

    def test_land_requeues_when_the_merged_result_fails_the_check(self):
        self.con.execute("UPDATE projects SET verify_cmd = 'test ! -f BREAK'")
        run = self.work('echo "changed by $PULSE_AGENT" >> README.md && git add -A && git commit -qm "worker change"')
        (self.repo / "BREAK").write_text("", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "main now fails the check together with the branch")
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["failed-verify"], results)
        self.assertNotIn("changed by aurora", (self.repo / "README.md").read_text(encoding="utf-8"), "nothing landed")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "queued")
        self.assertIn("verify exit 1", job["result"])
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'land.failed-verify'").fetchone()[0], 1)

    def done_job_with_branch(self, files, agent="aurora", title="older work", suffix=""):
        """A done job whose branch was cut before today's run-time guards existed: the files are committed by hand."""
        job, _ = pulse.add_job(self.con, "proj", "task", title, created_by="test", dedupe_source=False)
        branch = "pulse/%s/%s%s" % (agent, job["id"], suffix)
        git(self.repo, "branch", branch, "main")
        wt = self.tmp / ("wt-%s%s" % (job["id"], suffix))
        git(self.repo, "worktree", "add", "--detach", str(wt), branch)
        for name, text in files.items():
            (wt / name).parent.mkdir(parents=True, exist_ok=True)
            (wt / name).write_text(text, encoding="utf-8")
        git(wt, "add", "-A")
        git(wt, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", title)
        git(self.repo, "branch", "-f", branch, git(wt, "rev-parse", "HEAD").stdout.strip())
        git(self.repo, "worktree", "remove", "--force", str(wt))
        self.con.execute("UPDATE jobs SET status = 'done', branch = ?, finished_at = ? WHERE id = ?", (branch, NOW, job["id"]))
        return job["id"], branch

    def test_land_refuses_a_branch_that_changes_ci_or_owner_paths(self):
        job_id, _ = self.done_job_with_branch({".github/workflows/ci.yml": "on: push\n", "README.md": "also fine\n"})
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["refused"], results)
        self.assertFalse((self.repo / ".github" / "workflows" / "ci.yml").exists(), "nothing landed")
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "the refused branch is removed")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["branch"]), ("failed", ""), "the job is not retried: the change itself is refused")
        self.assertIn(".github/workflows/ci.yml", job["result"])
        self.assertEqual(len(list((self.home / "patches" / "proj").glob("*.patch"))), 1, "the patch is kept for a human")
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'land.refused'").fetchone()[0], 1)

    def test_land_runs_the_check_that_main_defines_not_the_branch_s(self):
        (self.repo / "check.sh").write_text("#!/bin/sh\ntest ! -f BREAK\n", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "the project's check")
        self.con.execute("UPDATE projects SET verify_cmd = 'sh check.sh'")
        # The branch breaks the code and rewrites the check so that it passes anyway.
        run = self.work('touch BREAK && printf "#!/bin/sh\\nexit 0\\n" > check.sh && git add -A && git commit -qm "green by decree"')
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["failed-verify"], results)
        self.assertIn("check run with main's check.sh", results[0][1])
        self.assertFalse((self.repo / "BREAK").exists(), "nothing landed")
        self.assertEqual((self.repo / "check.sh").read_text(encoding="utf-8"), "#!/bin/sh\ntest ! -f BREAK\n", "main's check is untouched")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "queued")
        # A branch that changes the check but leaves the code sound still lands; the note says whose check ran.
        self.work('printf "#!/bin/sh\\ntest ! -f BREAK && echo checked\\n" > check.sh && git add -A && git commit -qm "louder check"', agent="borealis")
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["landed"], results)
        self.assertIn("check run with main's check.sh", results[0][1])
        self.assertIn("echo checked", (self.repo / "check.sh").read_text(encoding="utf-8"), "the branch's own change is what landed")

    def test_land_stops_requeueing_after_the_limit_and_keeps_the_patch(self):
        self.con.execute("UPDATE projects SET verify_cmd = 'test ! -f BREAK'")
        run = self.work('echo "changed by $PULSE_AGENT" >> README.md && git add -A && git commit -qm "worker change"')
        (self.repo / "BREAK").write_text("", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "main fails its own check for now")
        job_id = run["job_id"]
        for attempt in range(pulse.LAND_REQUEUE_LIMIT):
            results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
            self.assertEqual([s for s, _ in results], ["failed-verify"], (attempt, results))
            self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (job_id,)).fetchone()[0], "queued")
            self.con.execute("UPDATE jobs SET status = 'done', branch = ? WHERE id = ?", (self.work_again(job_id, attempt), job_id))
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["failed-verify"], results)
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["branch"]), ("failed", ""), "the third failure ends the job instead of requeueing it")
        self.assertIn("not landed after 2 requeues", job["result"])
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'job.abandoned'").fetchone()[0], 1)
        self.assertTrue(list((self.home / "patches" / "proj").glob("*.patch")), "the patch stays")

    def work_again(self, job_id, attempt):
        """A fresh branch for the same job, as a rerun would leave it."""
        branch = "pulse/aurora/%s-retry%d" % (job_id, attempt)
        git(self.repo, "branch", branch, "main")
        wt = self.home / ("retry-%d" % attempt)
        git(self.repo, "worktree", "add", "--detach", str(wt), branch)
        (wt / "README.md").write_text("retry %d\n" % attempt, encoding="utf-8")
        git(wt, "add", "-A")
        git(wt, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "retry %d" % attempt)
        git(self.repo, "branch", "-f", branch, git(wt, "rev-parse", "HEAD").stdout.strip())
        git(self.repo, "worktree", "remove", "--force", str(wt))
        return branch

    def test_land_drops_a_committed_result_file_and_refuses_an_empty_branch(self):
        job_id, _ = self.done_job_with_branch({"README.md": "changed by an older worker\n", "PULSE_RESULT.md": "SUMMARY: x\n"})
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["landed"], results)
        self.assertIn("changed by an older worker", (self.repo / "README.md").read_text(encoding="utf-8"))
        self.assertFalse((self.repo / "PULSE_RESULT.md").exists(), "the result file never reaches main")
        self.assertEqual(git(self.repo, "ls-files", "PULSE_RESULT.md").stdout.strip(), "")
        self.assertIn("drop PULSE_RESULT.md", git(self.repo, "log", "-3", "--format=%s").stdout)
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (job_id,)).fetchone()[0], "done")
        job_id, _ = self.done_job_with_branch({"PULSE_RESULT.md": "SUMMARY: only\n"}, agent="borealis", title="empty work")
        results = pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["refused"], results)
        self.assertIn("nothing but PULSE_RESULT.md", results[0][1])
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (job_id,)).fetchone()[0], "failed")

    def test_landing_setting_human_keeps_branches_until_asked(self):
        self.work('echo "changed by $PULSE_AGENT" >> README.md && git add -A && git commit -qm "worker change"')
        pulse.set_setting(self.con, "landing", "human", "human")
        self.assertEqual(pulse.land_pending(self.con, self.project(), actor="daemon", now=NOW), [])
        self.assertTrue(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "the branch waits for a human")
        code, out, err = run_cli("land", "--project", "proj")
        self.assertEqual(code, 0, err)
        self.assertIn("landed", out)
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "")

    def test_land_command_takes_one_evolve_branch_without_a_job(self):
        git(self.repo, "checkout", "-q", "-b", "evolve/memory/one-lesson")
        (self.repo / "LESSON.md").write_text("kept\n", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", "kept variant")
        git(self.repo, "checkout", "-q", "main")
        code, out, err = run_cli("land", "--project", "proj", "--branch", "evolve/memory/one-lesson")
        self.assertEqual(code, 0, err + out)
        self.assertTrue((self.repo / "LESSON.md").is_file())
        self.assertEqual(git(self.repo, "branch", "--list", "evolve/*").stdout.strip(), "")
        code, out, err = run_cli("land", "--project", "proj", "--branch", "feature/other")
        self.assertNotEqual(code, 0, "only pulse/* and evolve/* branches land")

    def test_a_worker_never_migrates_the_shared_store(self):
        self.con.execute("UPDATE meta SET value = ? WHERE key = 'schema_version'", (str(pulse.SCHEMA_VERSION - 1),))
        self.con.commit()
        self.con.close()
        os.environ["PULSE_JOB"] = "job-test"
        try:
            with self.assertRaises(pulse.PulseError) as ctx:
                pulse.connect()
            self.assertIn("never migrates", str(ctx.exception))
            probe = sqlite3.connect(str(self.home / "pulse.sqlite"))
            self.assertEqual(probe.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()[0],
                             str(pulse.SCHEMA_VERSION - 1), "the store is untouched")
            probe.close()
        finally:
            os.environ.pop("PULSE_JOB", None)
        self.con = pulse.connect()
        self.assertEqual(self.con.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()["value"],
                         str(pulse.SCHEMA_VERSION), "the runner itself migrates as before")
        os.environ["PULSE_JOB"] = "job-test"
        try:
            pulse.connect().close()  # an up-to-date store is read fine from a worker
        finally:
            os.environ.pop("PULSE_JOB", None)


class RunnableCloneTestCase(StoreFixture):
    """WU-021: a throwaway clone is made runnable before the worker starts and before every check - the runner
    installs what the base commit's lockfile pins; a machine that lacks the tool loses only that project."""

    MISSING = "definitely-missing-tool-xyz"

    def setUp(self):
        super().setUp()
        self._rules = pulse.SETUP_RULES
        self._path = os.environ.get("PATH", "")
        self.tools = self.tmp / "tools"
        self.tools.mkdir()
        os.environ["PATH"] = "%s:%s" % (self.tools, self._path)

    def tearDown(self):
        pulse.SETUP_RULES = self._rules
        os.environ["PATH"] = self._path
        super().tearDown()

    def tool(self, name, body):
        script = self.tools / name
        script.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
        script.chmod(0o755)
        return script

    def commit_lockfile(self, name="fake.lock", repo=None):
        repo = repo or self.repo
        (repo / name).write_text("pinned\n", encoding="utf-8")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "add %s" % name)

    def use_rule(self, tool, command=None):
        pulse.SETUP_RULES = (("fake", "fake.lock", command or "%s install" % tool, tool),)

    def test_setup_command_reads_root_lockfiles_at_the_base_commit_only(self):
        self.assertEqual(pulse.setup_command(self.repo, "HEAD"), ("", []), "no lockfile, no setup")
        (self.repo / "sub").mkdir()
        (self.repo / "sub" / "package-lock.json").write_text("{}", encoding="utf-8")
        self.commit_lockfile("sub/package-lock.json")
        self.assertEqual(pulse.setup_command(self.repo, "HEAD"), ("", []), "a nested lockfile is not the project's")
        self.commit_lockfile("uv.lock")
        self.commit_lockfile("pnpm-lock.yaml")
        (self.repo / "poetry.lock").write_text("uncommitted\n", encoding="utf-8")  # only the working tree has it
        cmd, files = pulse.setup_command(self.repo, "HEAD")
        self.assertEqual(files, ["uv.lock", "pnpm-lock.yaml"], "one rule per ecosystem, the first lockfile of a group wins")
        self.assertTrue(cmd.startswith("export CI=1 "), cmd)
        self.assertIn("COREPACK_ENABLE_DOWNLOAD_PROMPT=0", cmd)
        self.assertIn("; uv sync --frozen --all-extras && corepack pnpm install --frozen-lockfile --prefer-offline", cmd)
        base = git(self.repo, "rev-parse", "HEAD~2").stdout.strip()
        self.assertEqual(pulse.setup_command(self.repo, base), ("", []), "detection follows the commit, not the tree")
        for lockfile in pulse.SETUP_LOCKFILES:
            self.assertIn(lockfile, pulse.CHECK_FILES, "a branch never changes what the check installs")

    def test_missing_tool_names_the_machine_not_the_job(self):
        cmd, _ = "export CI=1; uv sync --frozen --all-extras && corepack pnpm install --frozen-lockfile --prefer-offline", None
        self.assertEqual(pulse.missing_tool(cmd, 127, "bash: line 1: uv: command not found"), "uv")
        self.assertEqual(pulse.missing_tool(cmd, 127, "sh: corepack: not found"), "corepack")
        self.assertEqual(pulse.missing_tool(cmd, 127, ""), "uv", "exit 127 with no words still blames a tool")
        self.assertEqual(pulse.missing_tool(cmd, 1, "error: failed to resolve"), "", "a real failure is the job's")
        self.assertEqual(pulse.missing_tool("", 127, "x: command not found"), "")

    def test_setup_runs_in_the_clone_before_the_worker_and_the_brief_says_so(self):
        self.use_rule("fake-tool")
        self.tool("fake-tool", 'test "$1" = install && touch .installed && echo "fake-tool installed in $PWD"')
        self.commit_lockfile()
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'test -f .installed || { echo "setup did not run first" >&2; exit 1; }\n'
            'grep -q "fake-tool install" "$PULSE_BRIEF" || { echo "the brief does not name the setup" >&2; exit 1; }\n'
            'echo "changed by $PULSE_AGENT" >> README.md && git commit -qam "worker change"\n'
            'printf "SUMMARY: ran with dependencies installed\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        log = Path(run["log"]).read_text(encoding="utf-8")
        self.assertIn("# setup from fake.lock", log)
        self.assertIn("fake-tool installed in", log)
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {}, "nothing was missing")

    def test_a_missing_tool_keeps_only_that_project_off_the_machine_and_returns_the_attempt(self):
        self.use_rule(self.MISSING)
        self.commit_lockfile()
        self.con.execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'")
        other = make_repo(self.tmp, "other")
        pulse.add_project(self.con, str(other), verify_cmd="true", actor="human")
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('echo "changed" >> README.md && git commit -qam "c" && printf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        pulse.add_job(self.con, "other", "task", "tidy the README of the other project", created_by="human")
        self.con.execute("UPDATE jobs SET status = 'cancelled' WHERE project = 'other'")
        run, reason = pulse.work_once(self.con, "aurora", project="proj", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("needs `%s`" % self.MISSING, run["summary"])
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "queued", "the job waits for a machine that has the tool")
        self.assertEqual(job["attempts"], 0, "the attempt is returned: the machine failed, not the agent")
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {"proj": self.MISSING})
        self.assertEqual(self.con.execute("SELECT count(*) FROM posts WHERE kind = 'result'").fetchone()[0], 0, "no result for a machine's miss")
        findings = self.con.execute("SELECT * FROM posts WHERE kind = 'finding' AND thread = 'tools-wanted'").fetchall()
        self.assertEqual(len(findings), 1)
        self.assertTrue(findings[0]["summary"].startswith("local lacks %s for proj" % self.MISSING), findings[0]["summary"])
        self.assertIn("pulse executor probe local", findings[0]["body"])
        # the same project is not taken again here; another project still is
        job2, exrow, why = pulse.take_job(self.con, "aurora", project="proj", now=NOW)
        self.assertIsNone(job2)
        self.assertIn("lacks %s for proj" % self.MISSING, why)
        self.con.execute("UPDATE jobs SET status = 'queued' WHERE project = 'other'")
        job3, exrow, why = pulse.take_job(self.con, "aurora", now=NOW)
        self.assertIsNotNone(job3, why)
        self.assertEqual(job3["project"], "other", "the machine keeps every project it can still serve")
        self.con.execute("UPDATE jobs SET status = 'cancelled' WHERE project = 'other'")
        job4, exrow, why = pulse.take_job(self.con, "aurora", now=NOW)
        self.assertIsNone(job4)
        self.assertEqual(why, "no queued job", "other projects may still be served; proj alone is off this machine")
        self.con.execute("UPDATE projects SET status = 'paused' WHERE slug = 'other'")
        job5, exrow, why = pulse.take_job(self.con, "aurora", now=NOW)
        self.assertIsNone(job5)
        self.assertIn("lacks %s" % self.MISSING, why, "with nothing else to serve, the reason names the tool")
        self.con.execute("UPDATE projects SET status = 'active' WHERE slug = 'other'")
        # a second miss the same day adds no second finding; the mark expires by itself
        pulse.tools_wanted(self.con, pulse.brain_project(self.con), "local", self.MISSING, "proj", NOW)
        self.assertEqual(self.con.execute("SELECT count(*) FROM posts WHERE kind = 'finding' AND thread = 'tools-wanted'").fetchone()[0], 1)
        self.assertIn("lacks %s for proj" % self.MISSING, pulse.executors_line(self.con, NOW))
        later = pulse.plus_hours(NOW, pulse.UNFIT_HOURS + 1)
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, later), {}, "the runner tries again after %dh" % pulse.UNFIT_HOURS)
        self.assertNotIn("lacks", pulse.executors_line(self.con, later), "an expired mark is pruned from the store")

    def test_a_setup_failure_that_is_not_a_missing_tool_is_a_runner_error(self):
        self.use_rule("fake-tool")
        self.tool("fake-tool", 'echo "resolution failed: boom" >&2; exit 1')
        self.commit_lockfile()
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('printf "SUMMARY: never runs\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("setup failed on local (exit 1): resolution failed: boom", run["summary"])
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "queued", "a runner error goes back to the queue until attempts run out")
        self.assertEqual(job["attempts"], 1, "the attempt counts: the setup itself is broken")
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {})
        self.assertEqual(self.con.execute("SELECT count(*) FROM posts WHERE thread = 'tools-wanted'").fetchone()[0], 0)

    def test_a_check_that_cannot_start_on_the_machine_marks_it_unfit_and_requeues_the_job(self):
        self.con.execute("UPDATE projects SET verify_cmd = ?, brain = 1 WHERE slug = 'proj'", ("%s test" % self.MISSING,))
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('echo "changed by $PULSE_AGENT" >> README.md && git commit -qam "c" && printf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertIn("the check could not start on local: `%s` is not installed there" % self.MISSING, run["summary"])
        self.assertEqual(run["branch"], "", "unverified commits never land")
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 1), "back to the queue for a machine that can judge it; the attempt counts")
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {"proj": self.MISSING})
        finding = self.con.execute("SELECT * FROM posts WHERE kind = 'finding' AND thread = 'tools-wanted'").fetchone()
        self.assertTrue(finding["summary"].startswith("local lacks %s for proj" % self.MISSING), finding["summary"])
        self.assertEqual(pulse.check_tool_missing("make test", 127, "bash: line 1: make: command not found"), "make")
        self.assertEqual(pulse.check_tool_missing("make test", 2, "make: *** No rule to make target 'test'"), "", "a failing check is the branch's")
        self.assertEqual(pulse.check_tool_missing("CI=1 ./scripts/check.sh", 127, "check.sh: No such file or directory"), "check.sh")

    def test_landing_installs_in_the_worktree_and_defers_when_it_cannot(self):
        self.use_rule("fake-tool")
        marker = self.tmp / "installs"
        self.tool("fake-tool", 'touch .installed && echo "$PWD" >> "%s"' % marker)
        self.con.execute("UPDATE projects SET verify_cmd = 'test -f .installed' WHERE slug = 'proj'")
        self.commit_lockfile()
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('echo "changed by $PULSE_AGENT" >> README.md && git commit -qam "c" && printf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertEqual(marker.read_text(encoding="utf-8").count("\n"), 1, "installed once, in the clone")
        # the tool breaks before the landing: the branch waits instead of being blamed
        self.tool("fake-tool", 'echo "disk full" >&2; exit 1')
        project = pulse.project_rows(self.con)[0]
        results = pulse.land_pending(self.con, project, actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["deferred"], results)
        self.assertIn("setup from fake.lock failed", results[0][1])
        self.assertEqual(git(self.repo, "rev-parse", "--verify", "refs/heads/%s" % run["branch"]).returncode, 0, "the branch stays")
        self.assertNotIn("changed by aurora", (self.repo / "README.md").read_text(encoding="utf-8"))
        # the tool works again: the check runs in an installed worktree and the branch lands
        self.tool("fake-tool", 'touch .installed && echo "$PWD" >> "%s"' % marker)
        results = pulse.land_pending(self.con, project, actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["landed"], results)
        self.assertIn("changed by aurora", (self.repo / "README.md").read_text(encoding="utf-8"))
        self.assertEqual(marker.read_text(encoding="utf-8").count("\n"), 2, "installed again, in the landing worktree")
        self.assertFalse((self.repo / ".installed").exists(), "the real checkout was never installed into")


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


HANDS_SH = SCRIPTS / "hosted" / "pulse-hands.sh"

# An `ssh` that never leaves the machine: it drops the options and the host and hands the command string to
# pulse-hands exactly as sshd's forced command would (SSH_ORIGINAL_COMMAND). PULSE_SHIM_FAIL_VERB makes one verb
# fail like a dead connection (exit 255); PULSE_SHIM_FAIL=always refuses every call; PULSE_SHIM_OLD_HANDS=1 answers
# like hands installed before ADR-022 (no `key` verb, no keys= in the probe).
SSH_SHIM = r"""#!/bin/bash
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-i|-l|-F) shift 2 ;;
    -*) shift ;;
    *) break ;;
  esac
done
host="$1"; shift
echo "$host $*" >> "$PULSE_SHIM_LOG"
if [ "${PULSE_SHIM_FAIL:-}" = "always" ]; then echo "ssh: connect to host $host port 22: Connection refused" >&2; exit 255; fi
if [ -n "${PULSE_SHIM_FAIL_VERB:-}" ] && [[ "$1" == b64\ * ]]; then
  verb=$(printf '%s' "${1#b64 }" | base64 --decode | tr '\0' '\n' | head -1)
  if [ "$verb" = "$PULSE_SHIM_FAIL_VERB" ]; then echo "ssh: connection to $host closed by remote host" >&2; exit 255; fi
fi
if [ -n "${PULSE_SHIM_OLD_HANDS:-}" ] && [[ "$1" == b64\ * ]]; then
  verb=$(printf '%s' "${1#b64 }" | base64 --decode | tr '\0' '\n' | head -1)
  if [ "$verb" = "key" ]; then echo "pulse-hands: verb not allowed: key" >&2; exit 2; fi
  if [ "$verb" = "probe" ]; then export SSH_ORIGINAL_COMMAND="$*"; bash "$PULSE_HANDS" | sed -E 's/ keys=[^ ]*//'; exit "${PIPESTATUS[0]}"; fi
fi
export SSH_ORIGINAL_COMMAND="$*"
exec bash "$PULSE_HANDS"
"""


class HandsFixture(StoreFixture):
    """A remote executor over ssh, driven through the shim and pulse-hands.sh; no tests of its own."""

    def setUp(self):
        super().setUp()
        self.remote = self.tmp / "remote"
        self.remote.mkdir()
        shim_dir = self.tmp / "bin"
        shim_dir.mkdir()
        shim = shim_dir / "ssh"
        shim.write_text(SSH_SHIM, encoding="utf-8")
        shim.chmod(0o755)
        self._env_before = {k: os.environ.get(k) for k in ("PATH", "PULSE_HANDS", "PULSE_HANDS_WORKDIR", "PULSE_HANDS_ALLOW_ANY_WORKER",
                                                            "PULSE_HANDS_KEYS_OK", "PULSE_SHIM_LOG", "PULSE_SHIM_FAIL", "PULSE_SHIM_FAIL_VERB",
                                                            "PULSE_SHIM_OLD_HANDS", "SSH_ORIGINAL_COMMAND", "SSH_CONNECTION", "SSH_CLIENT")}
        # When this suite itself runs as a remote verify, sshd's forced-command request is in the environment;
        # the hands under test must read the test's argv, not that request.
        for key in ("SSH_ORIGINAL_COMMAND", "SSH_CONNECTION", "SSH_CLIENT"):
            os.environ.pop(key, None)
        os.environ["PATH"] = "%s:%s" % (shim_dir, os.environ.get("PATH", ""))
        os.environ["PULSE_HANDS"] = str(HANDS_SH)
        os.environ["PULSE_HANDS_WORKDIR"] = str(self.remote)
        os.environ["PULSE_HANDS_ALLOW_ANY_WORKER"] = "1"
        os.environ["PULSE_HANDS_KEYS_OK"] = "yes"
        os.environ["PULSE_SHIM_LOG"] = str(self.tmp / "shim.log")
        os.environ.pop("PULSE_SHIM_FAIL", None)
        os.environ.pop("PULSE_SHIM_FAIL_VERB", None)
        os.environ.pop("PULSE_SHIM_OLD_HANDS", None)

    def tearDown(self):
        for key, value in self._env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        super().tearDown()

    def add_hands(self, name="hands", projects="*", workers=1, activate=True):
        row = pulse.add_executor(self.con, name, "pulse@hands.example:2222", str(self.remote), workers=workers, projects=projects, now=NOW)
        self.assertEqual(row["status"], "drained", "new hands take no job until a probe passes")
        if activate:
            ok, note = pulse.probe_executor(self.con, name, actor=pulse.HUMAN_AGENT, now=NOW)
            self.assertTrue(ok, note)
            self.assertEqual(pulse.executor_row(self.con, name)["status"], "active")
        return pulse.executor_row(self.con, name)


class ExecutorTestCase(HandsFixture):
    """PLAN-020 slice 1: a remote executor over ssh, driven end to end through the shim and pulse-hands.sh."""

    def hands(self, *args, stdin=b"", cwd=None):
        return subprocess.run(["bash", str(HANDS_SH), *args], input=stdin, capture_output=True, cwd=cwd or str(self.tmp),
                              env=dict(os.environ, PULSE_HANDS_WORKDIR=str(self.remote)))

    def test_the_key_verb_takes_only_a_private_key_and_only_after_prepare(self):
        # ADR-022 §2: the hands accept the run's key once the clone exists, refuse anything that is not a key.
        bundle = self.tmp / "b.bundle"
        git(self.repo, "bundle", "create", "-q", str(bundle), "main")
        out = self.hands("key", "j1", "hands", stdin=b"-----BEGIN OPENSSH PRIVATE KEY-----\nx\n-----END OPENSSH PRIVATE KEY-----\n")
        self.assertEqual(out.returncode, 2)
        self.assertIn(b"no clone for job j1", out.stderr)
        out = self.hands("prepare", "j1", "main", "aurora", stdin=bundle.read_bytes())
        self.assertEqual(out.returncode, 0, out.stderr)
        out = self.hands("key", "j1", "hands", stdin=b"not a key\n")
        self.assertEqual(out.returncode, 2)
        self.assertIn(b"not an openssh private key", out.stderr)
        self.assertFalse((self.remote / "j1" / "run.key").exists(), "a refused key is not kept")
        out = self.hands("key", "j1", "bad name!", stdin=b"-----BEGIN OPENSSH PRIVATE KEY-----\n")
        self.assertIn(b"bad executor name", out.stderr)
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(self.tmp / "k")], check=True, capture_output=True)
        out = self.hands("key", "j1", "hands", stdin=(self.tmp / "k").read_bytes())
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout.strip(), b"keyed")
        key = self.remote / "j1" / "run.key"
        self.assertEqual(stat.S_IMODE(key.stat().st_mode), 0o600)
        clone = self.remote / "j1" / "clone"
        cfg = {k: subprocess.run(["git", "-C", str(clone), "config", "--get", k], capture_output=True, text=True).stdout.strip()
               for k in ("commit.gpgsign", "gpg.format", "gpg.ssh.program", "user.signingkey", "core.hooksPath")}
        self.assertEqual(cfg, {"commit.gpgsign": "true", "gpg.format": "ssh", "gpg.ssh.program": "ssh-keygen",
                               "user.signingkey": str(key), "core.hooksPath": str(clone / ".git" / "hooks")})
        hook = (clone / ".git" / "hooks" / "prepare-commit-msg").read_text()
        self.assertIn("--trailer 'Pulse-Agent: aurora' --trailer 'Pulse-Job: j1' --trailer 'Pulse-Executor: hands'", hook)
        # the probe names the job directory that holds a key; clean removes both
        self.assertIn("keys=j1", self.hands("probe").stdout.decode())
        self.assertEqual(self.hands("clean", "j1").returncode, 0)
        self.assertFalse(key.exists())
        self.assertIn("keys=-", self.hands("probe").stdout.decode())

    def test_a_remote_commit_by_another_author_lands_nothing(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script('echo x >> README.md\ngit -c user.name=w -c user.email=w@x commit -qam "as someone else"\n'
                                    'printf "SUMMARY: tried\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertIn("is authored by w <w@x>, not aurora", run["summary"])
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "", "nothing landed")
        self.assertEqual(git(self.repo, "for-each-ref", "refs/pulse/").stdout.strip(), "")
        self.assertEqual(sorted(p.name for p in self.remote.iterdir()), [], "the hands are clean after a refusal too")

    def test_a_probe_reports_a_signing_key_that_outlived_its_run(self):
        row = self.add_hands()
        # a key in a job directory the sweep could not remove (clean fails) is a problem the human sees
        (self.remote / "job-old").mkdir()
        (self.remote / "job-old" / "run.key").write_text("stale", encoding="utf-8")
        os.environ["PULSE_SHIM_FAIL_VERB"] = "clean"
        try:
            ok, note = pulse.probe_executor(self.con, "hands", now=NOW)
        finally:
            os.environ.pop("PULSE_SHIM_FAIL_VERB", None)
        self.assertFalse(ok, note)
        self.assertIn("1 signing key(s) left without a live run: job-old", note)
        # once the sweep can clean it, the probe is ok again and the key is gone
        ok, note = pulse.probe_executor(self.con, "hands", now=NOW)
        self.assertTrue(ok, note)
        self.assertFalse((self.remote / "job-old").exists())


    # -- WU-014: a machine fault drains the machine, never the job ----------

    def test_hands_that_predate_this_hub_are_drained_and_the_job_goes_back_whole(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script('printf "SUMMARY: fine\\n" > PULSE_RESULT.md')
        job_id = self.con.execute("SELECT id FROM jobs").fetchone()["id"]
        os.environ["PULSE_SHIM_OLD_HANDS"] = "1"
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("refuse `key`", run["summary"])
        self.assertIn("pulse pair <admin@host> --name hands", run["summary"])
        job = self.con.execute("SELECT status, attempts, lease_agent FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"], job["lease_agent"]), ("queued", 0, ""), "the attempt is returned")
        row = pulse.executor_row(self.con, "hands")
        self.assertEqual((row["status"], row["errors"]), ("drained", 1))
        self.assertTrue(row["probe_note"].startswith("fault: "), row["probe_note"])
        self.assertEqual(pulse.runs_today(self.con, NOW), 0)
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM events WHERE action = 'executor.drained'").fetchone()[0], 1)
        self.assertEqual(pulse.executor_slots(self.con, NOW), ["local"], "drained hands offer no slot")
        self.assertFalse(list((self.home / "work").glob("*.key")), "the run's key is gone with the run")
        # the next lease goes to this machine, and the job lands there
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual((run["status"], run["executor"]), ("done", "local"), run["summary"])
        # a probe of the old hands fails and names the refresh; a human probe of refreshed hands re-activates them
        ok, note = pulse.probe_executor(self.con, "hands", actor=pulse.HUMAN_AGENT, now=NOW)
        self.assertFalse(ok)
        self.assertIn("predate ADR-022", note)
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "drained")
        os.environ.pop("PULSE_SHIM_OLD_HANDS")
        ok, note = pulse.probe_executor(self.con, "hands", actor=pulse.HUMAN_AGENT, now=NOW)
        self.assertTrue(ok, note)
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "active")

    def test_a_worker_that_cannot_log_in_on_remote_hands_drains_them_not_the_job(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        unauth = self.worker_script('echo "Error: re-authenticate. If using a Fine-Grained PAT, ensure it has the Copilot Requests permission" >&2\nexit 1')
        job_id = self.con.execute("SELECT id FROM jobs").fetchone()["id"]
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=unauth, now=NOW, executor="hands")
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("rejected at launch", run["summary"])
        self.assertIn("the login on hands, not the job", run["summary"])
        self.assertIn("pulse executor login hands", run["summary"])
        job = self.con.execute("SELECT status, attempts FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 0))
        row = pulse.executor_row(self.con, "hands")
        self.assertEqual(row["status"], "drained")
        self.assertIn("fault: worker rejected at launch", row["probe_note"])
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_failed"], 0)
        self.assertIn("keys=-", self.hands("probe").stdout.decode(), "the job directory and its key were cleaned")
        # WU-016: the fault scheduled a recheck, and a shallow probe - even a human's - does not return the machine
        self.assertEqual(row["backoff_minutes"], 15)
        self.assertTrue(pulse.login_drained(row))
        ok, note = pulse.probe_executor(self.con, "hands", actor=pulse.HUMAN_AGENT, now=NOW)
        self.assertTrue(ok, note)
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "drained", "logged_in=yes is not a working login")
        # one real copilot call that answers is what returns it
        pulse.set_setting(self.con, "runner_cmd", self.worker_script("echo ok", "copilot-ok.sh"), "human")
        ok, note = pulse.login_check(self.con, "hands", actor="runner", now=NOW)
        self.assertTrue(ok, note)
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "active")
        # a launch rejection that is not about the login stays the runner's ordinary error: charged, hands kept
        badflag = self.worker_script('echo "error: option --max-ai-credits argument 5 is invalid" >&2\nexit 1', "badflag.sh")
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=badflag, now=NOW, executor="hands")
        self.assertEqual(run["status"], "error")
        job = self.con.execute("SELECT status, attempts FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 1))
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "active")

    def test_prepare_verifies_the_bundle_with_a_git_that_needs_a_repository(self):
        # git 2.43 (Ubuntu 24.04) refuses `git bundle verify` outside a repository; the forced command starts
        # in the hands' home, which is none, so every remote job used to die at prepare with "does not verify".
        real_git = shutil.which("git")
        wrapper = self.tmp / "bin" / "git"
        wrapper.write_text(
            "#!/bin/bash\n"
            "if [ \"$1\" = bundle ] && [ \"$2\" = verify ] && ! %s rev-parse --git-dir >/dev/null 2>&1; then\n"
            "  echo 'error: need a repository to verify a bundle' >&2; exit 1\n"
            "fi\n"
            "exec %s \"$@\"\n" % (shlex.quote(real_git), shlex.quote(real_git)), encoding="utf-8")
        wrapper.chmod(0o755)
        bundle = self.tmp / "source.bundle"
        self.assertEqual(git(self.repo, "bundle", "create", "-q", str(bundle), "main").returncode, 0)
        home = self.tmp / "hands-home"
        home.mkdir()
        done = subprocess.run(["bash", str(HANDS_SH), "prepare", "job-1", "main", "hands-1"], input=bundle.read_bytes(),
                              cwd=str(home), env=dict(os.environ), capture_output=True, timeout=120)
        self.assertEqual(done.returncode, 0, done.stderr.decode("utf-8", "replace"))
        base = done.stdout.decode("utf-8").strip().split()[-1]
        self.assertEqual(base, git(self.repo, "rev-parse", "main").stdout.strip())
        self.assertTrue((self.remote / "job-1" / "clone" / ".git").is_dir())
        self.assertEqual(sorted(p.name for p in (self.remote / "job-1").iterdir()), ["clone"], "no bundle or scratch repository left behind")
        # a broken bundle still dies the same way and leaves the job directory clean of scratch
        broken = subprocess.run(["bash", str(HANDS_SH), "prepare", "job-2", "main", "hands-1"], input=b"not a bundle",
                                cwd=str(home), env=dict(os.environ), capture_output=True, timeout=120)
        self.assertEqual(broken.returncode, 2)
        self.assertIn("the bundle does not verify", broken.stderr.decode("utf-8"))
        self.assertFalse((self.remote / "job-2" / "verify").exists())

    def test_executor_lifecycle_add_probe_list_drain_remove(self):
        with self.assertRaises(pulse.PulseError):
            pulse.add_executor(self.con, "bad", "no-user-here", "/srv/pulse")
        with self.assertRaises(pulse.PulseError):
            pulse.add_executor(self.con, "bad", "u@h", "relative/path")
        with self.assertRaises(pulse.PulseError):
            pulse.add_executor(self.con, "local", "u@h", "/srv/pulse")
        row = self.add_hands(projects="proj, Proj,other")
        self.assertEqual(row["projects"], "other,proj")
        self.assertTrue(pulse.executor_allows(row, "proj"))
        self.assertFalse(pulse.executor_allows(row, "third"))
        self.assertIn("ok:", row["probe_note"])
        self.assertIn("keys_ok=yes", row["probe_note"])
        code, out, err = run_cli("executor", "list")
        self.assertEqual(code, 0, err)
        self.assertIn("local", out)
        self.assertIn("hands", out)
        self.assertIn("pulse@hands.example:2222", out)
        code, out, err = run_cli("status")
        self.assertIn("executors", out)
        self.assertIn("hands active", out)
        # a probe that finds a loose key or a leftover remote fails and never activates
        os.environ["PULSE_HANDS_KEYS_OK"] = "no"
        ok, note = pulse.probe_executor(self.con, "hands", actor=pulse.HUMAN_AGENT, now=NOW)
        self.assertFalse(ok)
        self.assertIn("restrict", note)
        os.environ["PULSE_HANDS_KEYS_OK"] = "yes"
        code, out, err = run_cli("executor", "drain", "hands")
        self.assertEqual(code, 0, err)
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "drained")
        run, reason = pulse.work_once(self.con, pulse.spawn_agent(self.con, "r", "m")["name"], executor="hands", now=NOW)
        self.assertIsNone(run)
        self.assertIn("drained", reason)
        code, out, err = run_cli("executor", "activate", "hands")
        self.assertNotEqual(code, 0, "the last probe failed: activate refuses until a probe passes")
        code, out, err = run_cli("executor", "probe", "hands")
        self.assertEqual(code, 0, err)
        self.assertEqual(pulse.executor_row(self.con, "hands")["status"], "active")
        with self.assertRaises(pulse.PulseError):
            pulse.set_executor_status(self.con, "local", "drained")
        # a job directory a dead hub left on the hands is swept by the next passing probe, a busy one is kept
        (self.remote / "job-old" / "clone").mkdir(parents=True)
        (self.remote / "job-busy" / "clone").mkdir(parents=True)
        self.con.execute("INSERT INTO runs (id, job_id, agent, project, started_at, status, executor) VALUES ('run-b', 'job-busy', 'a', 'proj', ?, 'running', 'hands')", (NOW,))
        ok, note = pulse.probe_executor(self.con, "hands", actor=pulse.HUMAN_AGENT, now=NOW)
        self.assertTrue(ok, note)
        self.assertIn("jobs=swept:1", note)
        self.assertEqual(sorted(p.name for p in self.remote.iterdir()), ["job-busy"])
        self.con.execute("DELETE FROM runs WHERE id = 'run-b'")
        code, out, err = run_cli("executor", "remove", "hands")
        self.assertEqual(code, 0, err)
        self.assertIsNone(pulse.executor_row(self.con, "hands"))
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM events WHERE action LIKE 'executor.%'").fetchone()[0] > 3, True)

    def test_a_remote_run_lands_from_the_hands_casts_its_votes_and_leaves_nothing_behind(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.join_agent(self.con, "borealis", "r", "m", kind="worker")
        card = pulse.create_post(self.con, "suggestion", "proj", "evolution", "tidy", "shorten the README title; check:readme", agent="borealis")
        own = pulse.create_post(self.con, "suggestion", "proj", "evolution", "mine", "aurora's own card; check:x", agent="aurora")
        worker = self.worker_script(
            'test -z "$PULSE_HOME" && test -z "$PULSE_CLI"\n'
            'test -f "$PULSE_BRIEF" && grep -q "## Ballot" "$PULSE_BRIEF" && grep -q "%s" "$PULSE_BRIEF"\n'
            'echo "changed by $PULSE_AGENT on $(hostname)" >> README.md\n'
            'git commit -qam "worker change"\n'
            'printf "SUMMARY: appended one line\\nFINDING: remote hands work\\nVOTE: %s up the title is long\\n'
            'VOTE: %s up my own card\\nVOTE: 20990101T000000Z-nobody down not on the ballot\\n" > PULSE_RESULT.md' % (card["id"], card["id"], own["id"]))
        before = git(self.repo, "rev-parse", "HEAD").stdout.strip()
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", Path(run["log"]).read_text())
        self.assertEqual(run["executor"], "hands")
        self.assertEqual(run["commits"], 1)
        branch = "pulse/aurora/%s" % run["job_id"]
        self.assertEqual(run["branch"], branch)
        self.assertIn("changed by aurora", git(self.repo, "show", "%s:README.md" % branch).stdout)
        self.assertEqual(git(self.repo, "rev-parse", "HEAD").stdout.strip(), before, "the real checkout is untouched")
        self.assertEqual(git(self.repo, "status", "--porcelain").stdout, "")
        # the clone was committed as the agent, signed by this run's key, stamped with the job and the hands (ADR-022)
        body = git(self.repo, "log", "-1", "--format=%an <%ae>%n%(trailers)", branch).stdout
        self.assertIn("aurora <aurora@pulse.local>", body)
        self.assertIn("Pulse-Agent: aurora", body)
        self.assertIn("Pulse-Job: %s" % run["job_id"], body)
        self.assertIn("Pulse-Executor: hands", body)
        self.assertEqual(self.signature_status(branch, "aurora", run["signing_pubkey"]), "G")
        # nothing left on either side: job directory and its run.key on the hands, bundle and key on the hub
        self.assertEqual(sorted(p.name for p in self.remote.iterdir()), [], "the hands' job directory is removed")
        self.assertFalse((self.home / "work" / ("%s.bundle" % run["job_id"])).exists(), "the bundle is removed")
        self.assertFalse((self.home / "work" / ("%s.key" % run["job_id"])).exists(), "the hub's private key is removed")
        self.assertFalse((self.home / "work" / run["job_id"]).exists())
        shim_log = (self.tmp / "shim.log").read_text()
        key_call = base64.b64encode(b"".join(a.encode() + b"\0" for a in ("key", run["job_id"], "hands"))).decode()
        self.assertEqual(shim_log.count(key_call), 1, "the key crossed once, over the forced command, on stdin")
        self.assertNotIn("PRIVATE KEY", shim_log)
        # the hub pulled over ssh; the hands never learned the checkout path
        shim_log = (self.tmp / "shim.log").read_text()
        self.assertIn("git-upload-pack '%s/%s/clone'" % (self.remote, run["job_id"]), shim_log)
        self.assertNotIn(str(self.repo), shim_log)
        # the result names the executor, the finding is there, the votes were cast by the hub
        result = self.con.execute("SELECT * FROM posts WHERE kind = 'result'").fetchone()
        self.assertIn("executor `hands`", result["body"])
        self.assertIn("remote hands work", result["body"])
        self.assertIn("Votes: 1 cast of 3", result["body"])
        votes = self.con.execute("SELECT * FROM posts WHERE kind = 'vote'").fetchall()
        self.assertEqual(len(votes), 1, "one vote: the own card and the unknown id were skipped")
        self.assertEqual((votes[0]["on_post"], votes[0]["vote"], votes[0]["agent"]), (card["id"], "up", "aurora"))
        self.assertEqual(json.loads(votes[0]["extra"])["cast_by"], "hub-from-result")
        self.assertIn("the title is long", votes[0]["summary"])
        log = Path(run["log"]).read_text()
        self.assertIn("was not on this run's ballot", log)
        self.assertEqual(pulse.executor_row(self.con, "hands")["runs_done"], 1)
        self.assertEqual(pulse.runs_today(self.con, NOW), 1)

    def test_losing_the_hands_before_the_worker_is_an_error_after_it_is_a_failure(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        worker = self.worker_script('printf "SUMMARY: fine\\n" > PULSE_RESULT.md')
        job_id = self.con.execute("SELECT id FROM jobs").fetchone()["id"]
        os.environ["PULSE_SHIM_FAIL_VERB"] = "prepare"
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("unreachable before worker start", run["summary"])
        job = self.con.execute("SELECT status, attempts FROM jobs WHERE id = ?", (job_id,)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 1), "requeued: nothing was spent")
        self.assertEqual(pulse.runs_today(self.con, NOW), 0)
        self.assertEqual(pulse.executor_row(self.con, "hands")["errors"], 1)
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM posts WHERE kind = 'result'").fetchone()[0], 0)
        # three in a row: unreachable, with a backoff the daemon honours
        pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        row = pulse.executor_row(self.con, "hands")
        self.assertEqual((row["status"], row["errors"], row["backoff_minutes"]), ("unreachable", 3, 1))
        self.assertTrue(row["next_probe_at"] > NOW)
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNone(run)
        self.assertIn("unreachable", reason)
        self.assertEqual(pulse.executor_slots(self.con, NOW), ["local"], "an unreachable executor offers no slot")
        # not due yet: nothing probed; due and still dead: backoff doubles; alive again: back in service
        self.assertEqual(pulse.reprobe_due(self.con, 300, now=NOW), [])
        later = pulse.plus_hours(NOW, 2 / 60.0)
        os.environ["PULSE_SHIM_FAIL"] = "always"
        self.assertEqual(pulse.reprobe_due(self.con, 300, now=later), [])
        row = pulse.executor_row(self.con, "hands")
        self.assertEqual((row["status"], row["backoff_minutes"]), ("unreachable", 2))
        os.environ.pop("PULSE_SHIM_FAIL")
        os.environ.pop("PULSE_SHIM_FAIL_VERB")
        much_later = pulse.plus_hours(NOW, 1)
        back = pulse.reprobe_due(self.con, 300, now=much_later)
        self.assertEqual([n for n, _ in back], ["hands"])
        row = pulse.executor_row(self.con, "hands")
        self.assertEqual((row["status"], row["errors"], row["backoff_minutes"], row["next_probe_at"]), ("active", 0, 0, ""))
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (job_id,)).fetchone()["status"], "failed",
                         "three runner errors exhaust the job's attempts")
        # the connection dies after the worker started: the agent's failed run, cap spent, job failed
        job_id = pulse.add_job(self.con, "proj", "task", "again", created_by="test")[0]["id"]
        os.environ["PULSE_SHIM_FAIL_VERB"] = "run"
        run, why = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNotNone(run, why)
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertIn("unreachable after worker start", run["summary"])
        self.assertEqual(self.con.execute("SELECT status FROM jobs WHERE id = ?", (job_id,)).fetchone()["status"], "failed")
        self.assertEqual(pulse.runs_today(self.con, NOW), 1)
        self.assertEqual(pulse.agent_row(self.con, "aurora")["jobs_failed"], 1)
        self.assertEqual(pulse.executor_row(self.con, "hands")["runs_failed"], 1)
        results = [r["summary"] for r in self.con.execute("SELECT summary FROM posts WHERE kind = 'result' ORDER BY id")]
        self.assertEqual(len(results), 2, results)
        self.assertIn("runner errors", results[0], "the job that ran out of attempts is reported too")
        self.assertIn("unreachable after worker start", results[1])

    def test_remote_timeout_verify_failure_owner_paths_and_a_committed_result_file(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m", kind="worker")
        pulse.set_setting(self.con, "jobs_per_day", "0", "human")  # four jobs in one test day
        pulse.set_setting(self.con, "minutes_per_job", "0", "human")
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=self.worker_script("sleep 20"), now=NOW, executor="hands")
        self.assertEqual(run["status"], "timeout", run["summary"])
        self.assertEqual(sorted(p.name for p in self.remote.iterdir()), [], "cleaned after a timeout too")
        pulse.set_setting(self.con, "minutes_per_job", "10", "human")
        # verify fails on the hands: nothing lands
        self.con.execute("UPDATE projects SET verify_cmd = 'grep -q never README.md' WHERE slug = 'proj'")
        pulse.add_job(self.con, "proj", "task", "verify me", created_by="test")
        worker = self.worker_script('echo x >> README.md\ngit commit -qam x\nprintf "SUMMARY: x\\n" > PULSE_RESULT.md')
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertIn("verify exit 1", run["summary"])
        self.assertEqual(git(self.repo, "branch", "--list", "pulse/*").stdout.strip(), "")
        self.con.execute("UPDATE projects SET verify_cmd = 'true' WHERE slug = 'proj'")
        # owner-owned paths never land from the hands either
        pulse.add_job(self.con, "proj", "task", "owner paths", created_by="test")
        worker = self.worker_script('mkdir -p docs/evolution && echo stop > docs/evolution/STOP\ngit add -A\ngit commit -qm stop\n'
                                    'printf "SUMMARY: stopped it\\n" > PULSE_RESULT.md')
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertEqual(run["status"], "failed", run["summary"])
        self.assertIn("owner-owned", run["summary"])
        # a committed PULSE_RESULT.md is dropped with one more commit before landing
        pulse.add_job(self.con, "proj", "task", "committed result", created_by="test")
        worker = self.worker_script('echo y >> README.md\nprintf "SUMMARY: y\\n" > PULSE_RESULT.md\ngit add -A\ngit commit -qm y')
        run, _ = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertEqual(run["commits"], 2)
        self.assertNotEqual(git(self.repo, "cat-file", "-e", "%s:PULSE_RESULT.md" % run["branch"]).returncode, 0)

    def test_the_daemon_refills_a_freed_slot_while_another_run_is_in_flight(self):
        """WU-019: the loop no longer waits for every slot; a slot refills as soon as its run ends."""
        self.add_hands("hands-a", projects="*")
        self.add_hands("hands-b", projects="*")
        pulse.set_setting(self.con, "workers", "0", "human")
        pulse.set_setting(self.con, "parallel_per_project", "4", "human")
        pulse.set_setting(self.con, "jobs_per_day", "0", "human")
        for j in self.con.execute("SELECT id FROM jobs WHERE status = 'queued'").fetchall():
            pulse.finish_job(self.con, j["id"], "human", "cancelled")
        pulse.add_job(self.con, "proj", "task", "slow one", created_by="test")
        pulse.add_job(self.con, "proj", "task", "quick one", created_by="test")
        pulse.add_job(self.con, "proj", "task", "quick two", created_by="test")
        worker = self.worker_script('if grep -q "slow one" "$PULSE_BRIEF"; then sleep 4; fi\n'
                                    'printf "SUMMARY: done by $PULSE_AGENT\\n" > PULSE_RESULT.md')
        out = io.StringIO()
        began = time.time()
        with unittest.mock.patch.object(pulse, "DISPATCH_TICK_SECONDS", 1), \
                unittest.mock.patch.object(pulse, "plan_jobs", return_value=[]):
            self.assertEqual(pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=3, out=out), "done")
        elapsed = time.time() - began
        runs = {r["job_id"]: r for r in self.con.execute("SELECT r.*, j.title FROM runs r JOIN jobs j ON j.id = r.job_id").fetchall()}
        by_title = {r["title"]: r for r in runs.values()}
        self.assertEqual(sorted(by_title), ["quick one", "quick two", "slow one"], out.getvalue())
        self.assertEqual({r["status"] for r in by_title.values()}, {"done"}, out.getvalue())
        self.assertLess(by_title["quick two"]["started_at"], by_title["slow one"]["finished_at"],
                        "the second quick job started while the slow one was still running")
        self.assertLess(elapsed, 9, "no loop waited for the slow run before refilling the other slot")
        self.assertEqual(len({r["agent"] for r in by_title.values()}), 3, "every run in flight has its own worker")
        self.assertEqual(out.getvalue().count(" on hands-"), 3, out.getvalue())
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM runs WHERE status = 'running'").fetchone()[0], 0,
                         "max_loops drains the runs in flight before returning")
        note = json.loads(self.con.execute("SELECT value FROM meta WHERE key = 'capacity'").fetchone()["value"])
        self.assertEqual(note["slots"], 2)

    def test_the_daemon_fills_every_free_slot_and_honours_project_limits(self):
        other = make_repo(self.tmp, "other")
        code, out, err = run_cli("init", "--path", str(other), "--verify", "true")
        self.assertEqual(code, 0, err)
        self.add_hands("hands-a", projects="proj")
        self.add_hands("hands-b", projects="*")
        pulse.set_setting(self.con, "workers", "0", "human")  # nothing runs on this machine while hands exist
        self.assertEqual(pulse.executor_slots(self.con, NOW), ["hands-a", "hands-b"])
        worker = self.worker_script('sleep 1\nprintf "SUMMARY: hi from $PULSE_AGENT\\n" > PULSE_RESULT.md')
        out = io.StringIO()
        self.assertEqual(pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=1, out=out), "done")
        runs = self.con.execute("SELECT * FROM runs ORDER BY executor").fetchall()
        self.assertEqual([r["status"] for r in runs], ["done", "done"], out.getvalue())
        self.assertEqual([r["executor"] for r in runs], ["hands-a", "hands-b"])
        self.assertEqual(runs[0]["project"], "proj", "hands-a serves proj only")
        self.assertEqual(runs[1]["project"], "other")
        self.assertNotEqual(runs[0]["agent"], runs[1]["agent"], "parallel slots use distinct workers")
        self.assertIn("on hands-a", out.getvalue())
        # an executor limited to one project refuses another
        run, reason = pulse.work_once(self.con, runs[0]["agent"], project="other", executor="hands-a", now=NOW)
        self.assertIsNone(run)
        self.assertIn("does not serve other", reason)
        # with no remote hands the machine always keeps one slot, even at workers 0
        pulse.remove_executor(self.con, "hands-a")
        pulse.remove_executor(self.con, "hands-b")
        self.assertEqual(pulse.executor_slots(self.con, NOW), ["local"])
        # a run a dead runner left behind is closed at daemon start and frees its slot
        self.con.execute("INSERT INTO runs (id, job_id, agent, project, started_at, status, executor) VALUES ('run-x', 'j', 'a', 'proj', ?, 'running', 'local')", (NOW,))
        pulse.set_setting(self.con, "workers", "1", "human")
        self.assertEqual(pulse.executor_slots(self.con, NOW), [])
        out = io.StringIO()
        pulse.daemon(self.con, interval=0, worker_cmd=worker, max_loops=1, out=out)
        self.assertEqual(self.con.execute("SELECT status FROM runs WHERE id = 'run-x'").fetchone()["status"], "lost")
        self.assertIn("closed 1 run(s)", out.getvalue())

    def test_the_brief_carries_the_ballot_and_result_lines_carry_votes(self):
        pulse.join_agent(self.con, "aurora", "r", "m")
        job = self.con.execute("SELECT * FROM jobs").fetchone()
        project = pulse.project_by_slug(self.con, "proj")
        ballot = [{"id": "20260901T000000Z-borealis", "key": "evolution/tidy", "up": 1, "down": 0, "summary": "shorten the title"}]
        remote = pulse.write_brief(job, project, "aurora", "/srv/pulse/j/clone", 10, ballot=ballot, remote=True)
        self.assertIn("## Ballot", remote)
        self.assertIn("20260901T000000Z-borealis", remote)
        self.assertIn("VOTE:", remote)
        self.assertIn("no PULSE store", remote)
        self.assertNotIn("$PULSE_CLI", remote)
        local = pulse.write_brief(job, project, "aurora", "/tmp/clone", 10, ballot=[], remote=False)
        self.assertIn("$PULSE_CLI", local)
        self.assertIn("Nothing awaits your vote", local)
        parsed = pulse.parse_result_text("SUMMARY: did it\n- VOTE: abc up   because it helps\nVOTE: def down\n`VOTE:` ghi UP: shouting\nFINDING: f\n")
        self.assertEqual(parsed["votes"], [("abc", "up", "because it helps"), ("def", "down", ""), ("ghi", "up", "shouting")])
        self.assertEqual(parsed["summary"], "did it")
        self.assertEqual(parsed["findings"], ["f"])
        self.assertFalse(pulse.parse_result_text(None)["found"])

    def test_a_version_one_store_gains_the_executor_table_and_column_in_place(self):
        self.con.execute("UPDATE meta SET value = '1' WHERE key = 'schema_version'")
        self.con.execute("DROP TABLE executors")
        self.con.execute("ALTER TABLE runs DROP COLUMN executor")
        self.con.close()
        self.con = pulse.connect()
        self.assertEqual(self.con.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()["value"], str(pulse.SCHEMA_VERSION))
        cols = {r["name"] for r in self.con.execute("PRAGMA table_info(runs)")}
        self.assertIn("executor", cols)
        self.assertIn("signing_pubkey", cols, "ADR-022: the run row carries the public half of its key")
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM executors").fetchone()[0], 0)
        self.assertEqual([r["subject"] for r in self.con.execute("SELECT subject FROM events WHERE action = 'schema.upgrade' ORDER BY seq")],
                         ["2", "3"], "one upgrade event per version climbed")
        self.con.close()
        self.con = pulse.connect()  # idempotent
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM events WHERE action = 'schema.upgrade'").fetchone()[0], 2)

    def test_the_command_channel_is_argv_not_a_shell_string(self):
        row = {"address": "pulse@hands.example:2222"}
        cmd = pulse.ssh_command(row, ["verify", "j", "10", "--", "echo $(id); rm -rf /"])
        self.assertEqual(cmd[-2], "pulse@hands.example")
        self.assertIn("-p", cmd)
        self.assertEqual(cmd[cmd.index("-p") + 1], "2222")
        self.assertTrue(cmd[-1].startswith("b64 "))
        self.assertNotIn("$", cmd[-1])
        # the hands refuse a plain shell string and an unknown verb
        env = dict(os.environ, SSH_ORIGINAL_COMMAND="probe; touch %s/pwned" % self.tmp)
        res = subprocess.run(["bash", str(HANDS_SH)], env=env, capture_output=True, text=True)
        self.assertEqual(res.returncode, 2)
        self.assertIn("refused", res.stderr)
        self.assertFalse((self.tmp / "pwned").exists())
        env["SSH_ORIGINAL_COMMAND"] = "b64 " + base64.b64encode(b"shutdown\0now\0").decode()
        res = subprocess.run(["bash", str(HANDS_SH)], env=env, capture_output=True, text=True)
        self.assertEqual(res.returncode, 2)
        self.assertIn("verb not allowed", res.stderr)
        # the worker must be copilot unless the test switch is set, env keys are whitelisted, upload-pack stays under the workdir
        env.pop("PULSE_HANDS_ALLOW_ANY_WORKER", None)
        env["SSH_ORIGINAL_COMMAND"] = "git-upload-pack '/etc'"
        res = subprocess.run(["bash", str(HANDS_SH)], env=env, capture_output=True, text=True)
        self.assertEqual(res.returncode, 2)
        self.assertIn("under", res.stderr)


class RunnableCloneRemoteTestCase(HandsFixture):
    """WU-021 over ssh: the setup runs through the hands' `verify` verb; a machine that lacks a tool stays active
    for its other projects and takes the project back after a human probe."""

    MISSING = "definitely-missing-tool-xyz"

    def setUp(self):
        super().setUp()
        self._rules = pulse.SETUP_RULES
        self.tools = self.tmp / "tools"
        self.tools.mkdir()
        os.environ["PATH"] = "%s:%s" % (self.tools, os.environ.get("PATH", ""))

    def tearDown(self):
        pulse.SETUP_RULES = self._rules
        super().tearDown()

    def prepare(self, tool, body=None):
        pulse.SETUP_RULES = (("fake", "fake.lock", "%s install" % tool, tool),)
        if body is not None:
            script = self.tools / tool
            script.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
            script.chmod(0o755)
        (self.repo / "fake.lock").write_text("pinned\n", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "add fake.lock")
        pulse.join_agent(self.con, "aurora", "r", "m")
        self.add_hands()

    def test_setup_runs_on_the_machine_before_the_worker(self):
        self.prepare("fake-tool", 'touch .installed && echo "installed in $PWD"')
        worker = self.worker_script(
            'test -f .installed || { echo "setup did not run first" >&2; exit 1; }\n'
            'echo "changed by $PULSE_AGENT" >> README.md && git commit -qam "worker change"\n'
            'printf "SUMMARY: ran with dependencies installed\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        log = Path(run["log"]).read_text(encoding="utf-8")
        self.assertIn("# setup from fake.lock", log)
        self.assertIn("installed in %s" % (self.remote / run["job_id"] / "clone"), log, "installed in the remote clone")
        verbs = []
        for line in (self.tmp / "shim.log").read_text(encoding="utf-8").splitlines():
            if " b64 " in line:
                verbs.append(base64.b64decode(line.split(" b64 ", 1)[1].split()[0]).split(b"\0")[0].decode())
        self.assertLess(verbs.index("verify"), verbs.index("brief"), "the setup precedes the brief and the worker: %s" % verbs)

    def test_a_missing_tool_keeps_the_machine_active_and_a_human_probe_gives_the_project_back(self):
        self.prepare(self.MISSING)
        self.con.execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'")
        worker = self.worker_script('printf "SUMMARY: never runs\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("needs `%s`" % self.MISSING, run["summary"])
        row = pulse.executor_row(self.con, "hands")
        self.assertEqual(row["status"], "active", "a missing tool is not a fault: the machine keeps its other projects")
        self.assertEqual(int(row["errors"]), 0)
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 0))
        self.assertEqual(pulse.unfit_projects(self.con, "hands", NOW), {"proj": self.MISSING})
        finding = self.con.execute("SELECT * FROM posts WHERE kind = 'finding' AND thread = 'tools-wanted'").fetchone()
        self.assertIsNotNone(finding)
        self.assertTrue(finding["summary"].startswith("hands lacks %s for proj" % self.MISSING), finding["summary"])
        self.assertFalse((self.remote / run["job_id"]).exists(), "the job directory is cleaned like any run's")
        _, _, why = pulse.take_job(self.con, "aurora", now=NOW, executor="hands")
        self.assertIn("hands lacks %s" % self.MISSING, why)
        self.assertIn("lacks %s for proj" % self.MISSING, pulse.executors_line(self.con, NOW))
        # the daemon's own probe changes nothing; the human's probe, after installing the tool, does
        ok, note = pulse.probe_executor(self.con, "hands", actor="daemon", now=NOW)
        self.assertTrue(ok, note)
        self.assertEqual(pulse.unfit_projects(self.con, "hands", NOW), {"proj": self.MISSING})
        ok, note = pulse.probe_executor(self.con, "hands", actor=pulse.HUMAN_AGENT, now=NOW)
        self.assertTrue(ok, note)
        self.assertEqual(pulse.unfit_projects(self.con, "hands", NOW), {})
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'executor.fit'").fetchone()[0], 1)
        job2, _, why = pulse.take_job(self.con, "aurora", now=NOW, executor="hands")
        self.assertIsNotNone(job2, why)
        self.assertEqual(job2["id"], run["job_id"], "the same job is taken there again")


class RecipeTestCase(StoreFixture):
    """ADR-026 slice 1: the environment recipe at docs/context/environment.md, read at the base commit, decides
    what a clone needs; the local machine installs pins with mise; recipe changes travel alone and wait for a
    human landing; `pulse env check` reads it all without touching anything."""

    MISSING = "definitely-missing-tool-xyz"
    TEMPLATE = (Path(__file__).resolve().parent.parent.parent / "context" / "_template-environment.md")

    def setUp(self):
        super().setUp()
        self._rules = pulse.SETUP_RULES
        self._path = os.environ.get("PATH", "")
        self.tools = self.tmp / "tools"
        self.tools.mkdir()
        os.environ["PATH"] = "%s:%s" % (self.tools, self._path)
        self.mise_log = self.tmp / "mise.log"

    def tearDown(self):
        pulse.SETUP_RULES = self._rules
        os.environ["PATH"] = self._path
        super().tearDown()

    def tool(self, name, body):
        script = self.tools / name
        script.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
        script.chmod(0o755)
        return script

    def bare_path(self):
        """A PATH with the fake tools, git, and the system shells only - never a real mise, wherever this runs."""
        dirs = [str(self.tools)]
        for name in ("git", "sh", "env"):
            found = shutil.which(name, path=self._path)
            if found and str(Path(found).parent) not in dirs:
                dirs.append(str(Path(found).parent))
        for system in ("/usr/bin", "/bin"):
            if system not in dirs:
                dirs.append(system)
        return ":".join(dirs)

    def fake_mise(self, fail_pin=""):
        """A stand-in mise: `install` logs its argv (and fails on `fail_pin`), `exec` runs the command after `--`
        with MISE_WRAPPED=1 so setup, check, and worker can prove they ran under it."""
        return self.tool("mise",
                         'case "$1" in\n'
                         '  install) shift; echo "install $*" >> %s; env | grep "^MISE_" | sort >> %s;\n'
                         '           for p in "$@"; do [ "$p" = "%s" ] && { echo "mise ERROR failed to install %s"; exit 1; }; done; exit 0 ;;\n'
                         '  exec) shift; while [ "$1" != "--" ]; do shift; done; shift; MISE_WRAPPED=1; export MISE_WRAPPED; exec "$@" ;;\n'
                         '  version) echo 2026.9.1 ;;\n'
                         'esac' % (shlex.quote(str(self.mise_log)), shlex.quote(str(self.mise_log)), fail_pin, fail_pin))

    def commit_recipe(self, text, repo=None, message="recipe"):
        repo = repo or self.repo
        path = repo / pulse.RECIPE_PATH
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", message)
        return git(repo, "rev-parse", "HEAD").stdout.strip()

    def commit_lockfile(self, name="fake.lock"):
        (self.repo / name).write_text("pinned\n", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "add %s" % name)

    def use_rule(self, tool):
        pulse.SETUP_RULES = (("fake", "fake.lock", "%s install" % tool, tool),)

    def set_verify(self, cmd):
        self.con.execute("UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'", (cmd,))

    def recipe(self, **keys):
        lines = ["---", "recipe: 1"]
        for key in ("tools", "setup", "services"):
            if key in keys:
                lines.append("%s: []" % key if not keys[key] else "%s:" % key)
                lines += ["  - %s" % item for item in keys[key]]
        if "env" in keys:
            lines.append("env: {}" if not keys["env"] else "env:")
            lines += ["  %s: %s" % kv for kv in keys["env"].items()]
        if "suggested_check" in keys:
            lines.append("suggested_check: %s" % keys["suggested_check"])
        return "\n".join(lines + ["---", "", "# Environment", ""])

    # -- the parser --------------------------------------------------------

    def test_parser_accepts_the_template_and_refuses_what_the_decision_names(self):
        recipe, problems = pulse.parse_recipe(self.TEMPLATE.read_text(encoding="utf-8"))
        self.assertEqual(problems, [], "the shipped template is a valid recipe")
        self.assertEqual(recipe["recipe"], "1")
        self.assertEqual(recipe["tools"], [])
        self.assertNotIn("setup", recipe, "the template leaves setup to the lockfile heuristic")
        self.assertEqual(recipe["env"], {})
        self.assertEqual(recipe["services"], [])
        good, problems = pulse.parse_recipe(self.recipe(tools=["node@22.1.0", "uv@0.8.3"], setup=["uv sync --frozen", "'npm ci'"],
                                                        env={"CI": '"1"', "APP_HOME": "/opt/app/some/very/long/path/that/goes/on/and/on/and/on"},
                                                        services=[], suggested_check="make test"))
        self.assertEqual(problems, [])
        self.assertEqual(good["tools"], ["node@22.1.0", "uv@0.8.3"])
        self.assertEqual(good["setup"], ["uv sync --frozen", "npm ci"])
        self.assertEqual(good["env"], {"CI": "1", "APP_HOME": "/opt/app/some/very/long/path/that/goes/on/and/on/and/on"})
        self.assertEqual(good["suggested_check"], "make test")
        spaced, problems = pulse.parse_recipe(self.recipe(env={"CFLAGS": '"-O2 -g"', "PLAIN": "a b c"}))
        self.assertEqual((problems, spaced["env"]), ([], {"CFLAGS": "-O2 -g", "PLAIN": "a b c"}), "values with spaces are allowed")
        self.assertEqual(pulse.recipe_env_exports(spaced), "export CFLAGS='-O2 -g' PLAIN='a b c'; ", "and reach the shell as one word each")
        secret_value = "hunter2" + "x" * 40
        refused = {
            "unknown key": "---\nrecipe: 1\ncolor: blue\n---\n",
            "latest": self.recipe(tools=["node@latest"]).replace("node@latest", "node"),
            "13 tools": self.recipe(tools=["t%d@1.0" % i for i in range(13)]),
            "pin with a space": self.recipe(tools=["node@22 1"]),
            "pin with ;": self.recipe(tools=["node@22;rm"]),
            "GITHUB_TOKEN name": self.recipe(env={"GITHUB_TOKEN": "abc"}),
            "API_KEY_NAME name": self.recipe(env={"API_KEY_NAME": "abc"}),
            "40-char value": self.recipe(env={"OPAQUE": secret_value}),
            "url credentials": self.recipe(env={"DATABASE": "postgres://user:hunter2@db/app"}),
            "jwt": self.recipe(env={"SESSION": "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnopqrstuvwxyz"}),
            "multi-line value": "---\nrecipe: 1\nenv:\n  GREETING: hello\n    world\n---\n",
            "recipe: 2": "---\nrecipe: 2\n---\n",
            "no frontmatter": "# Environment\n",
            "lowercase env": self.recipe(env={"greeting": "hi"}),
            "setup as scalar": "---\nrecipe: 1\nsetup: make\n---\n",
            "anchor": "---\nrecipe: 1\nsuggested_check: &x make\n---\n",
        }
        for label, text in refused.items():
            recipe, problems = pulse.parse_recipe(text)
            self.assertTrue(problems, "%s should be refused: %r" % (label, recipe))
            joined = " ".join(problems)
            self.assertNotIn("hunter2", joined, "%s: a problem echoed an env value" % label)
            self.assertNotIn(secret_value, joined)
        self.assertIn("`tools`: `node` is not an exact `name@version` pin", pulse.parse_recipe(refused["latest"])[1])
        self.assertTrue(any("GITHUB_TOKEN" in p and "secret" in p for p in pulse.parse_recipe(refused["GITHUB_TOKEN name"])[1]))
        self.assertTrue(any("at most 12" in p for p in pulse.parse_recipe(refused["13 tools"])[1]))

    def test_read_recipe_reads_the_named_commit_and_absent_means_none(self):
        self.assertEqual(pulse.read_recipe(self.repo, "HEAD"), (None, []))
        first = self.commit_recipe(self.recipe(tools=["node@22.1.0"]))
        self.commit_recipe(self.recipe(tools=["node@23.0.0"]), message="bump")
        self.assertEqual(pulse.read_recipe(self.repo, first)[0]["tools"], ["node@22.1.0"], "the base commit's recipe, not the tip's")
        self.assertEqual(pulse.read_recipe(self.repo, "HEAD")[0]["tools"], ["node@23.0.0"])
        self.commit_recipe("---\nrecipe: 2\n---\n", message="broken")
        recipe, problems = pulse.read_recipe(self.repo, "HEAD")
        self.assertTrue(problems)

    # -- run_job --------------------------------------------------------------

    def test_setup_from_the_recipe_replaces_the_heuristic_and_env_reaches_setup_check_and_worker(self):
        self.use_rule("fake-tool")
        self.tool("fake-tool", 'touch .heuristic-ran')
        self.commit_lockfile()
        self.commit_recipe(self.recipe(setup=['test -n "$GREETING" && touch .recipe-setup'], env={"GREETING": "quiet-value-7"}))
        self.set_verify('test -n "$GREETING" && test -f .recipe-setup')
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'test -f .recipe-setup || { echo "recipe setup did not run" >&2; exit 1; }\n'
            'test ! -f .heuristic-ran || { echo "the heuristic ran although the recipe has setup:" >&2; exit 1; }\n'
            'test "$GREETING" = quiet-value-7 || { echo "env did not reach the worker" >&2; exit 1; }\n'
            'test "$COPILOT_SUBAGENT_MAX_CONCURRENT" = 256 || { echo "no sub-agent cap in the worker env" >&2; exit 1; }\n'
            'test "$COPILOT_SUBAGENT_MAX_DEPTH" = 128 || { echo "no sub-agent depth in the worker env" >&2; exit 1; }\n'
            'grep -q "touch .recipe-setup" "$PULSE_BRIEF" || { echo "the brief does not name the setup" >&2; exit 1; }\n'
            'grep -q "variables are set for you: GREETING" "$PULSE_BRIEF" || { echo "the brief does not name the env" >&2; exit 1; }\n'
            'grep -q "quiet-value-7" "$PULSE_BRIEF" && { echo "the brief echoed an env value" >&2; exit 1; }\n'
            'grep -q "travels alone" "$PULSE_BRIEF" || { echo "the brief lacks the travel-alone rule" >&2; exit 1; }\n'
            'echo "changed by $PULSE_AGENT" >> README.md && git commit -qam "worker change"\n'
            'printf "SUMMARY: ran inside the recipe\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"] + "\n" + Path(run["log"]).read_text(encoding="utf-8"))
        log = Path(run["log"]).read_text(encoding="utf-8")
        self.assertIn("# setup from %s" % pulse.RECIPE_PATH, log)
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {})

    def test_an_empty_setup_list_runs_nothing_and_keeps_a_lockfile_project_off_no_machine(self):
        self.use_rule(self.MISSING)
        self.commit_lockfile()
        self.commit_recipe(self.recipe(setup=[]))
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('echo x >> README.md && git commit -qam "c" && printf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"])
        self.assertNotIn("# setup from", Path(run["log"]).read_text(encoding="utf-8"))
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {}, "nothing ran, so nothing was missing")

    def test_tool_pins_install_through_mise_and_wrap_setup_check_and_worker(self):
        self.fake_mise()
        self.commit_recipe(self.recipe(tools=["node@22.1.0", "uv@0.8.3"], setup=['test "$MISE_WRAPPED" = 1 && touch .setup-under-mise']))
        self.set_verify('test "$MISE_WRAPPED" = 1 && test -f .setup-under-mise')
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script(
            'test "$MISE_WRAPPED" = 1 || { echo "the worker did not run under mise exec" >&2; exit 1; }\n'
            'test "$MISE_YES" = 1 || { echo "MISE_YES unset" >&2; exit 1; }\n'
            'test "$MISE_IGNORED_CONFIG_PATHS" = "$PULSE_CLONE" || { echo "the clone\'s own mise files are not ignored" >&2; exit 1; }\n'
            'grep -q "node@22.1.0, uv@0.8.3" "$PULSE_BRIEF" || { echo "the brief does not name the pins" >&2; exit 1; }\n'
            'echo x >> README.md && git commit -qam "c" && printf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"] + "\n" + Path(run["log"]).read_text(encoding="utf-8"))
        mise_log = self.mise_log.read_text(encoding="utf-8")
        self.assertIn("install node@22.1.0 uv@0.8.3", mise_log, "one argv element per pin")
        self.assertIn("MISE_YES=1", mise_log)
        self.assertIn("MISE_IGNORED_CONFIG_PATHS=", mise_log)
        self.assertIn("$ mise install node@22.1.0 uv@0.8.3  # tools from %s" % pulse.RECIPE_PATH, Path(run["log"]).read_text(encoding="utf-8"))

    def test_missing_mise_and_a_failing_pin_keep_the_project_off_this_machine_with_one_finding_each(self):
        self.con.execute("UPDATE projects SET brain = 1 WHERE slug = 'proj'")
        self.commit_recipe(self.recipe(tools=["node@22.1.0"]))
        pulse.join_agent(self.con, "aurora", "r", "m")
        worker = self.worker_script('printf "SUMMARY: never runs\\n" > PULSE_RESULT.md')
        with unittest.mock.patch.dict(os.environ, {"PATH": self.bare_path()}):
            run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("`mise` is not installed", run["summary"])
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {"proj": pulse.RECIPE_UNFIT_MISE})
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual((job["status"], job["attempts"]), ("queued", 0), "the machine failed, not the agent")
        finding = self.con.execute("SELECT * FROM posts WHERE kind = 'finding' AND thread = 'tools-wanted'").fetchone()
        self.assertTrue(finding["summary"].startswith("local lacks mise for proj"), finding["summary"])
        self.assertIn("mise.run", finding["body"])
        _, _, why = pulse.take_job(self.con, "aurora", project="proj", now=NOW)
        self.assertIn("lacks mise for proj", why)
        self.assertIn("lacks mise for proj", pulse.executors_line(self.con, NOW))
        # a human probe of the local machine is not a thing; the mark expires, and mise is there now but one pin fails
        pulse.clear_unfit(self.con, pulse.LOCAL_EXECUTOR)
        self.fake_mise(fail_pin="node@22.1.0")
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("mise could not install node@22.1.0", run["summary"])
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {"proj": "tool:node@22.1.0"})
        findings = self.con.execute("SELECT summary, body FROM posts WHERE kind = 'finding' AND thread = 'tools-wanted' ORDER BY id").fetchall()
        self.assertEqual(len(findings), 2)
        self.assertTrue(findings[1]["summary"].startswith("local lacks node@22.1.0 for proj"), findings[1]["summary"])
        self.assertIn("mise install node@22.1.0", findings[1]["body"])
        self.assertIn("lacks node@22.1.0 for proj", pulse.executors_line(self.con, NOW))

    def test_an_invalid_recipe_errors_before_the_worker_and_posts_one_finding_a_day(self):
        self.commit_recipe("---\nrecipe: 1\nenv:\n  GITHUB_TOKEN: ghp_notreallyatoken12345678\n---\n")
        pulse.join_agent(self.con, "aurora", "r", "m")
        marker = self.tmp / "worker-ran"
        worker = self.worker_script('touch %s; printf "SUMMARY: never\\n" > PULSE_RESULT.md' % shlex.quote(str(marker)))
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("is not a valid recipe", run["summary"])
        self.assertIn("GITHUB_TOKEN", run["summary"])
        self.assertNotIn("ghp_", run["summary"], "the value is never echoed")
        self.assertFalse(marker.exists(), "no worker started")
        findings = self.con.execute("SELECT * FROM posts WHERE kind = 'finding' AND area = 'runner' AND thread = 'environment' AND project = 'proj'").fetchall()
        self.assertEqual(len(findings), 1)
        self.assertTrue(findings[0]["summary"].startswith("invalid environment recipe"), findings[0]["summary"])
        self.assertNotIn("ghp_", findings[0]["body"])
        job = self.con.execute("SELECT * FROM jobs WHERE id = ?", (run["job_id"],)).fetchone()
        self.assertEqual(job["status"], "queued", "a runner error; the job waits for the fix")
        run2, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW)
        self.assertIsNotNone(run2, reason)
        self.assertEqual(run2["status"], "error")
        self.assertEqual(self.con.execute("SELECT count(*) FROM posts WHERE kind = 'finding' AND thread = 'environment'").fetchone()[0], 1,
                         "one finding a day, not one per attempt")
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'env.invalid'").fetchone()[0], 1)

    def test_services_keep_the_project_off_every_machine_before_placement(self):
        self.commit_recipe(self.recipe(services=["postgres", "redis"]))
        pulse.join_agent(self.con, "aurora", "r", "m")
        self.assertEqual(pulse.recipe_blocks(self.con, pulse.LOCAL_EXECUTOR), {"proj": pulse.RECIPE_UNFIT_SERVICES})
        job, exrow, why = pulse.take_job(self.con, "aurora", project="proj", now=NOW)
        self.assertIsNone(job)
        self.assertIn("needs services for proj", why)
        job, exrow, why = pulse.take_job(self.con, "aurora", now=NOW)
        self.assertIsNone(job)
        self.assertIn("needs services", why)
        self.assertIn("needs services for proj", pulse.executors_line(self.con, NOW))
        # a job that reaches run_job anyway (leased before the recipe landed) ends the same way, with the mark
        leased = pulse.lease_job(self.con, "aurora", project="proj", now=NOW)
        self.assertIsNotNone(leased)
        worker = self.worker_script('printf "SUMMARY: never runs\\n" > PULSE_RESULT.md')
        run = pulse.run_job(self.con, leased, "aurora", worker_cmd=worker, now=NOW)
        self.assertEqual(run["status"], "error", run["summary"])
        self.assertIn("lists services (postgres, redis)", run["summary"])
        self.assertEqual(pulse.unfit_projects(self.con, pulse.LOCAL_EXECUTOR, NOW), {"proj": pulse.RECIPE_UNFIT_SERVICES})
        code, out, err = run_cli("env", "check", "--now", NOW)
        self.assertEqual(code, 1, out + err)
        self.assertIn("services postgres, redis", out)
        self.assertIn("needs services", out)
        # an empty list clears it without any mark to expire
        self.commit_recipe(self.recipe(services=[]), message="services provided elsewhere")
        pulse.clear_unfit(self.con, pulse.LOCAL_EXECUTOR)
        self.assertEqual(pulse.recipe_blocks(self.con, pulse.LOCAL_EXECUTOR), {})
        code, out, err = run_cli("env", "check", "--now", NOW)
        self.assertEqual(code, 0, out + err)

    # -- landing ----------------------------------------------------------------

    def branch_with(self, name, files, message="change"):
        """A branch off main with the given {path: text} written and committed, main left where it was."""
        git(self.repo, "checkout", "-q", "-b", name)
        for path, text in files.items():
            (self.repo / path).parent.mkdir(parents=True, exist_ok=True)
            (self.repo / path).write_text(text, encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", message)
        git(self.repo, "checkout", "-q", "main")
        return name

    def test_a_recipe_branch_travels_alone_and_waits_for_a_human_landing(self):
        self.commit_recipe(self.recipe(tools=[]))
        pulse.join_agent(self.con, "aurora", "r", "m")
        project = pulse.project_by_slug(self.con, "proj")
        mixed = self.branch_with("pulse/aurora/job-mixed", {pulse.RECIPE_PATH: self.recipe(tools=["node@22.1.0"]), "README.md": "more\n"})
        state, note = pulse.land_branch(self.con, project, mixed, "daemon", now=NOW)
        self.assertEqual(state, "refused", note)
        self.assertIn("recipe changes travel alone", note)
        self.assertNotEqual(git(self.repo, "rev-parse", "--verify", "--quiet", "refs/heads/%s" % mixed).returncode, 0, "a refused branch goes")
        alone = self.branch_with("pulse/aurora/job-recipe", {pulse.RECIPE_PATH: self.recipe(tools=["node@22.1.0"])})
        job, _ = pulse.add_job(self.con, "proj", "task", "fill the recipe", created_by="human")
        self.con.execute("UPDATE jobs SET status = 'done', branch = ? WHERE id = ?", (alone, job["id"]))
        state, note = pulse.land_branch(self.con, project, alone, "daemon", job=dict(self.con.execute("SELECT * FROM jobs WHERE id = ?", (job["id"],)).fetchone()), now=NOW)
        self.assertEqual(state, "held", note)
        self.assertIn("pulse land --project proj --branch %s" % alone, note)
        self.assertEqual(git(self.repo, "rev-parse", "--verify", "--quiet", "refs/heads/%s" % alone).returncode, 0, "a held branch stays")
        self.assertEqual(git(self.repo, "rev-parse", "main").stdout, git(self.repo, "rev-parse", "HEAD").stdout, "main did not move")
        questions = self.con.execute("SELECT * FROM posts WHERE kind = 'question' AND area = 'runner' AND thread = 'environment'").fetchall()
        self.assertEqual(len(questions), 1)
        self.assertIn(alone, questions[0]["summary"])
        self.assertIn("tools node@22.1.0", questions[0]["summary"])
        self.assertIn("+  - node@22.1.0", questions[0]["body"], "the question carries the diff")
        self.assertEqual(self.con.execute("SELECT branch, status FROM jobs WHERE id = ?", (job["id"],)).fetchone()[0], alone, "the job keeps its branch")
        # the daemon's landing pass holds it again without a second question, and lands nothing
        self.con.execute("UPDATE settings SET value = 'agents' WHERE name = 'landing'")
        results = pulse.land_pending(self.con, project, actor="daemon", now=NOW)
        self.assertEqual([s for s, _ in results], ["held"])
        self.assertEqual(self.con.execute("SELECT count(*) FROM posts WHERE kind = 'question' AND thread = 'environment'").fetchone()[0], 1)
        self.assertEqual(pulse.held_landings(self.con, "proj", self.repo), [alone])
        code, out, err = run_cli("env", "check", "--project", "proj", "--now", NOW)
        self.assertIn("held: %s" % alone, out)
        # an agent naming the branch is still held; the human naming it (no PULSE_AGENT) lands it
        with unittest.mock.patch.dict(os.environ, {"PULSE_AGENT": "aurora"}):
            code, out, err = run_cli("land", "--project", "proj", "--branch", alone)
        self.assertIn("held", out, "an agent cannot name a recipe branch into landing")
        self.assertEqual(self.con.execute("SELECT count(*) FROM posts WHERE kind = 'question' AND thread = 'environment'").fetchone()[0], 1)
        self.assertNotIn("PULSE_AGENT", os.environ)
        code, out, err = run_cli("land", "--project", "proj", "--branch", alone)
        self.assertEqual(code, 0, out + err)
        self.assertIn("landed", out)
        self.assertIn("check run with main's %s" % pulse.RECIPE_PATH, out)
        self.assertEqual(pulse.read_recipe(self.repo, "main")[0]["tools"], ["node@22.1.0"])
        self.assertEqual(self.con.execute("SELECT count(*) FROM events WHERE action = 'env.held'").fetchone()[0], 1,
                         "one event with the one question; the daemon's later passes hold quietly")
        self.assertEqual(pulse.held_landings(self.con, "proj", self.repo), [], "landed branches leave the held list")

    def test_the_landing_check_runs_with_the_base_recipe_when_the_branch_changed_it(self):
        self.commit_recipe(self.recipe(setup=["touch .base-setup"]))
        self.set_verify("test -f .base-setup && test ! -f .branch-setup")
        project = pulse.project_by_slug(self.con, "proj")
        alone = self.branch_with("pulse/aurora/job-r2", {pulse.RECIPE_PATH: self.recipe(setup=["touch .branch-setup"])})
        state, note = pulse.land_branch(self.con, project, alone, pulse.HUMAN_AGENT, now=NOW, by_hand=True)
        self.assertEqual(state, "landed", note)
        self.assertIn("check run with main's %s" % pulse.RECIPE_PATH, note)
        self.assertEqual(pulse.read_recipe(self.repo, "main")[0]["setup"], ["touch .branch-setup"], "the merge keeps the branch's recipe")

    # -- onboarding, env check, pair ---------------------------------------------

    def test_onboard_scaffolds_the_recipe_once_and_the_brief_asks_the_agent_to_fill_it(self):
        other = make_repo(self.tmp, "other")
        code, out, err = run_cli("onboard", str(other), "--verify", "true")
        self.assertEqual(code, 0, err)
        dest = other / pulse.RECIPE_PATH
        self.assertTrue(dest.is_file())
        self.assertEqual(dest.read_text(encoding="utf-8"), self.TEMPLATE.read_text(encoding="utf-8"))
        self.assertIn("scaffolded %s" % pulse.RECIPE_PATH, out)
        self.assertIn("?? docs/", git(other, "status", "--short").stdout, "never staged")
        dest.write_text(self.recipe(tools=["go@1.24.0"]), encoding="utf-8")
        code, out, err = run_cli("onboard", str(other), "--verify", "true")
        self.assertEqual(code, 0, err)
        self.assertIn("%s already present, untouched" % pulse.RECIPE_PATH, out)
        self.assertEqual(pulse.parse_recipe(dest.read_text(encoding="utf-8"))[0]["tools"], ["go@1.24.0"], "never overwritten")
        brief = self.con.execute("SELECT brief FROM jobs WHERE project = 'other' AND kind = 'onboard'").fetchone()[0]
        self.assertIn(pulse.RECIPE_PATH, brief)
        self.assertIn("Commit the recipe alone", brief)
        self.assertIn("Never write a token", brief)
        scaffolded = [json.loads(r["detail"])["scaffolded"] for r in self.con.execute("SELECT detail FROM events WHERE action = 'project.onboard' AND subject = 'other' ORDER BY seq")]
        self.assertIn(pulse.RECIPE_PATH, scaffolded[0])
        self.assertNotIn(pulse.RECIPE_PATH, scaffolded[1])

    def test_env_check_reports_recipes_machines_and_exits_one_when_a_machine_is_unfit(self):
        code, out, err = run_cli("env", "check", "--now", NOW)
        self.assertEqual(code, 0, out + err)
        self.assertIn("no %s" % pulse.RECIPE_PATH, out)
        self.commit_recipe(self.recipe(tools=["node@22.1.0"], env={"GREETING": "hello"}, suggested_check="make test"))
        with unittest.mock.patch.dict(os.environ, {"PATH": self.bare_path()}):
            code, out, err = run_cli("env", "check", "--project", "proj", "--now", NOW)
        self.assertEqual(code, 1, "no mise on this PATH: the local machine is unfit\n" + out + err)
        self.assertIn("recipe ok; tools node@22.1.0; setup: heuristic; env GREETING", out)
        self.assertNotIn("hello", out, "values never shown")
        self.assertIn("suggested_check `make test` (the store's verify command `true` decides)", out)
        self.assertIn("lacks mise", out)
        self.fake_mise()
        code, out, err = run_cli("env", "check", "--project", "proj", "--json", "--now", NOW)
        self.assertEqual(code, 0, out + err)
        report = json.loads(out)
        self.assertEqual(report["unfit"], [])
        self.assertEqual(report["projects"][0]["env"], ["GREETING"])
        self.assertEqual(report["projects"][0]["executors"][0], {"executor": "local", "fit": True, "why": "", "phrase": "", "hint": ""})
        self.commit_recipe("---\nrecipe: 3\n---\n", message="broken")
        code, out, err = run_cli("env", "check", "--project", "proj", "--now", NOW)
        self.assertIn("INVALID %s" % pulse.RECIPE_PATH, out)
        code, out, err = run_cli("env", "check", "--executor", "nowhere")
        self.assertNotEqual(code, 0)
        self.assertIn("No executor nowhere", err)

    def test_pair_scripts_and_cloud_init_carry_the_same_pinned_mise(self):
        apply = pulse.pair_apply_script("hands-2", "/srv/pulse", "ssh-ed25519 AAAATEST pulse-hub", "#!/bin/sh\necho hands\n", False)
        for needle in (pulse.MISE_VERSION, pulse.MISE_SHA256["x86_64"], pulse.MISE_SHA256["aarch64"], "sha256sum", "step mise failed"):
            self.assertIn(needle, apply)
        inspect = pulse.pair_inspect_script("/srv/pulse")
        self.assertIn("mise=missing", inspect)
        cloud_init = (Path(__file__).resolve().parent.parent / "hosted" / "cloud-init.yaml").read_text(encoding="utf-8")
        for needle in (pulse.MISE_VERSION, pulse.MISE_SHA256["x86_64"], pulse.MISE_SHA256["aarch64"]):
            self.assertIn(needle, cloud_init, "cloud-init and pulse pair install the same release")
        hands = HANDS_SH.read_text(encoding="utf-8")
        for key, value in pulse.WORKER_ENV:
            self.assertIn("%s=\"${%s:-%s}\"" % (key, key, value), hands, "the hands give a worker the same sub-agent caps as the hub")
        self.assertRegex(pulse.MISE_SHA256["x86_64"], r"^[0-9a-f]{64}$")


class RecipeRemoteTestCase(HandsFixture):
    """ADR-026 slice 1 on remote hands: pins and env wait for hands that can take them; setup alone still runs."""

    def test_remote_hands_are_unfit_hands_outdated_for_pins_or_env_before_placement(self):
        self.add_hands()
        pulse.join_agent(self.con, "aurora", "r", "m")
        path = self.repo / pulse.RECIPE_PATH
        path.parent.mkdir(parents=True)
        path.write_text("---\nrecipe: 1\ntools:\n  - node@22.1.0\n---\n", encoding="utf-8")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "recipe with pins")
        self.assertEqual(pulse.recipe_blocks(self.con, "hands"), {"proj": pulse.RECIPE_UNFIT_HANDS})
        self.assertEqual(pulse.recipe_blocks(self.con, pulse.LOCAL_EXECUTOR), {}, "the hub itself can take pins")
        job, exrow, why = pulse.take_job(self.con, "aurora", now=NOW, executor="hands")
        self.assertIsNone(job)
        self.assertIn("hands outdated for proj", why)
        self.assertIn("pulse pair", why)
        self.assertIn("hands outdated for proj", pulse.executors_line(self.con, NOW))
        code, out, err = run_cli("env", "check", "--project", "proj", "--executor", "hands", "--now", NOW)
        self.assertEqual(code, 1, out + err)
        self.assertIn("hands outdated", out)
        self.assertIn("pulse pair", out)
        path.write_text("---\nrecipe: 1\nenv:\n  GREETING: hello\n---\n", encoding="utf-8")
        git(self.repo, "commit", "-qam", "recipe with env")
        self.assertEqual(pulse.recipe_blocks(self.con, "hands"), {"proj": pulse.RECIPE_UNFIT_HANDS})
        # setup alone runs on the hands through the verify string
        path.write_text("---\nrecipe: 1\nsetup:\n  - touch .remote-setup\n---\n", encoding="utf-8")
        git(self.repo, "commit", "-qam", "recipe with setup only")
        self.assertEqual(pulse.recipe_blocks(self.con, "hands"), {})
        self.con.execute("UPDATE projects SET verify_cmd = 'test -f .remote-setup' WHERE slug = 'proj'")
        worker = self.worker_script('test -f .remote-setup || { echo "setup did not run on the hands" >&2; exit 1; }\n'
                                    'echo x >> README.md && git commit -qam "c" && printf "SUMMARY: ok\\n" > PULSE_RESULT.md')
        run, reason = pulse.work_once(self.con, "aurora", worker_cmd=worker, now=NOW, executor="hands")
        self.assertIsNotNone(run, reason)
        self.assertEqual(run["status"], "done", run["summary"] + "\n" + Path(run["log"]).read_text(encoding="utf-8"))
        self.assertIn("# setup from %s" % pulse.RECIPE_PATH, Path(run["log"]).read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
