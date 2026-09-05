#!/usr/bin/env python3
"""Stress test: concurrent lease take/renew race reproduction.

This test attempts to reproduce lease races by launching multiple threads that
try to lease the same queued job at the same time. The atomic lease_job
implementation must ensure at most one successful lease per round. The test
runs several rounds to measure flakiness: every round must have exactly one
winner.
"""

import importlib.util
import os
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent


def load_module(name, filename):
    spec = importlib.util.spec_from_file_location(name, SCRIPTS / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

pulse = load_module("pulse", "pulse.py")

GIT_ENV = dict(GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), "-c", "commit.gpgsign=false"] + list(args),
                          capture_output=True, text=True, env=dict(os.environ, **GIT_ENV))


class LeaseStressTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="pulse-lease-stress-"))
        self.home = self.tmp / "home"
        os.environ["PULSE_HOME"] = str(self.home)
        # create a minimal repo like tests do
        self.repo = self.tmp / "proj"
        self.repo.mkdir(parents=True)
        (self.repo / "AGENTS.md").write_text("# rules\n", encoding="utf-8")
        (self.repo / "README.md").write_text("hello\n", encoding="utf-8")
        subprocess.run(["git", "-C", str(self.repo), "init", "-q"], check=True, capture_output=True)
        git(self.repo, "checkout", "-q", "-B", "main")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", "init")
        # register project in the store
        code = pulse.main(["init", "--path", str(self.repo), "--verify", "true"])
        # connect
        self.con = pulse.connect()

    def tearDown(self):
        try:
            self.con.close()
        except Exception:
            pass
        shutil.rmtree(self.tmp, ignore_errors=True)

    def test_concurrent_lease_exclusive(self):
        # One job only; many agents compete. Exactly one lease must be granted.
        pulse.join_agent(self.con, "leader", "r", "m")
        # create worker agents
        agents = [f"w{i}" for i in range(8)]
        for a in agents:
            pulse.join_agent(self.con, a, "r", "m")
        job, _ = pulse.add_job(self.con, "proj", "task", "stress", "stress test")

        rounds = 30
        winners = 0
        for r in range(rounds):
            # reset job to queued if it was taken in a previous round
            self.con.execute("UPDATE jobs SET status = 'queued', lease_agent = '', lease_until = '', generation = 0 WHERE id = ?", (job["id"],))
            barrier = threading.Barrier(len(agents))
            results = []

            def try_lease(agent_name):
                # each thread uses its own DB connection like separate runners would
                con = pulse.connect()
                try:
                    barrier.wait()
                except Exception:
                    pass
                leased = pulse.lease_job(con, agent_name, job_id=job["id"], minutes=1, now=None)
                results.append((agent_name, leased is not None))
                con.close()

            threads = []
            for a in agents:
                t = threading.Thread(target=try_lease, args=(a,))
                t.start()
                threads.append(t)
            for t in threads:
                t.join()
            round_winners = [a for a, ok in results if ok]
            # exactly one thread should have leased the job
            self.assertEqual(len(round_winners), 1, f"round {r} had {len(round_winners)} winners: {round_winners}")
            winners += 1
            # mark the job finished so next round can reuse it
            lease_holder = round_winners[0]
            # finish the job as the holder using a fresh connection
            con2 = pulse.connect()
            # find current generation
            j = con2.execute("SELECT * FROM jobs WHERE id = ?", (job["id"],)).fetchone()
            pulse.finish_job(con2, job["id"], lease_holder, "queued", generation=j["generation"])
            con2.close()
        self.assertEqual(winners, rounds, "all rounds produced exactly one winner")


if __name__ == "__main__":
    unittest.main()
