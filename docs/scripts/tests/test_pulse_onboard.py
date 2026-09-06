#!/usr/bin/env python3
"""Tests for `pulse onboard` and the ADR-021 first-reflect planner rule."""

import json
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
import test_pulse as tp


class OnboardTestCase(tp.StoreFixture):
    def setUp(self):
        self._agent_before = os.environ.get("PULSE_AGENT")
        super().setUp()

    def tearDown(self):
        try:
            if self._agent_before is None:
                os.environ.pop("PULSE_AGENT", None)
            else:
                os.environ["PULSE_AGENT"] = self._agent_before
        finally:
            super().tearDown()

    def jobs_for(self, slug):
        return [dict(r) for r in self.con.execute(
            "SELECT * FROM jobs WHERE project = ? ORDER BY priority DESC, kind", (slug,))]

    def test_onboard_fresh_checkout_registers_scaffolds_and_queues_both_jobs_once(self):
        repo = tp.make_repo(self.tmp, "fresh-app")
        code, out, err = tp.run_cli("onboard", str(repo), "--verify", "true")
        self.assertEqual(code, 0, err)
        project = tp.pulse.project_by_slug(self.con, "fresh-app")
        self.assertIsNotNone(project)
        jobs = self.jobs_for("fresh-app")
        self.assertEqual([(j["kind"], j["priority"], j["source"]) for j in jobs],
                         [("onboard", 20, "onboard:fresh-app"), ("reflect", 19, "onboard-reflect:fresh-app")])
        evolution = repo / "docs" / "evolution"
        self.assertEqual(sorted(p.name for p in evolution.iterdir()), ["archive.md", "constitution.md", "evaluators.md"])
        self.assertIn("loop: off", (evolution / "constitution.md").read_text(encoding="utf-8"))
        self.assertEqual(tp.git(repo, "status", "--porcelain", "docs/evolution").stdout, "?? docs/evolution/\n")
        self.assertEqual(tp.git(repo, "diff", "--cached", "--name-only").stdout, "")
        self.assertIn("record the baseline", out)
        self.assertIn("set loop: on", out)
        self.assertIn("scaffolded docs/evolution/", out)

        first_ids = [j["id"] for j in jobs]
        code, out, err = tp.run_cli("onboard", str(repo), "--verify", "true")
        self.assertEqual(code, 0, err)
        self.assertIn("refreshed project fresh-app", out)
        self.assertIn("already queued onboarding job", out)
        self.assertIn("first reflect already exists", out)
        self.assertEqual([j["id"] for j in self.jobs_for("fresh-app")], first_ids)
        self.assertEqual(sorted(p.name for p in evolution.iterdir()), ["archive.md", "constitution.md", "evaluators.md"])

    def test_onboard_partial_evolution_folder_refuses_before_registering_or_writing(self):
        repo = tp.make_repo(self.tmp, "partial-app")
        evolution = repo / "docs" / "evolution"
        evolution.mkdir(parents=True)
        (evolution / "constitution.md").write_text("owner draft\n", encoding="utf-8")
        code, out, err = tp.run_cli("onboard", str(repo))
        self.assertEqual(code, 2)
        self.assertIn("missing evaluators.md, archive.md", err)
        self.assertIsNone(tp.pulse.project_by_slug(self.con, "partial-app"))
        self.assertEqual(sorted(p.name for p in evolution.iterdir()), ["constitution.md"])

    def test_onboard_complete_evolution_folder_is_left_untouched(self):
        repo = tp.make_repo(self.tmp, "complete-app")
        evolution = repo / "docs" / "evolution"
        evolution.mkdir(parents=True)
        contents = {
            "constitution.md": "owner constitution\nloop: on\n",
            "evaluators.md": "owner evaluators\n",
            "archive.md": "owner archive\n",
        }
        for name, text in contents.items():
            (evolution / name).write_text(text, encoding="utf-8")
        code, out, err = tp.run_cli("onboard", str(repo))
        self.assertEqual(code, 0, err)
        for name, text in contents.items():
            self.assertEqual((evolution / name).read_text(encoding="utf-8"), text)
        self.assertIn("docs/evolution/ already present, untouched", out)
        self.assertNotIn("scaffolded docs/evolution/", out)

    def test_init_still_registers_only_the_onboarding_job(self):
        jobs = self.jobs_for("proj")
        self.assertEqual([(j["kind"], j["priority"], j["source"]) for j in jobs],
                         [("onboard", 20, "onboard:proj")])
        self.assertFalse((self.repo / "docs" / "evolution").exists())

    def test_agent_gate_refuses_without_decision_before_registering(self):
        repo = tp.make_repo(self.tmp, "gated-app")
        os.environ["PULSE_AGENT"] = "worker-a"
        code, out, err = tp.run_cli("onboard", str(repo))
        self.assertEqual(code, 2)
        self.assertIn("no decided proposal names it", err)
        self.assertIsNone(tp.pulse.project_by_slug(self.con, "gated-app"))
        self.assertFalse((repo / "docs" / "evolution").exists())

    def test_agent_gate_accepts_human_decision_and_records_it_on_event(self):
        repo = tp.make_repo(self.tmp, "approved-app")
        post = tp.pulse.create_post(self.con, "suggestion", "proj", "governance", "onboard",
                                    "allow approved app", "Please onboard it.\n\nonboard: approved-app\n",
                                    agent=tp.pulse.HUMAN_AGENT)
        tp.pulse.decide(self.con, post["id"], "yes", "go ahead")
        os.environ["PULSE_AGENT"] = "worker-a"
        code, out, err = tp.run_cli("onboard", str(repo), "--now", tp.NOW)
        self.assertEqual(code, 0, err)
        self.assertIsNotNone(tp.pulse.project_by_slug(self.con, "approved-app"))
        event = self.con.execute(
            "SELECT detail FROM events WHERE action = 'project.onboard' AND subject = 'approved-app' ORDER BY seq DESC LIMIT 1"
        ).fetchone()
        self.assertEqual(json.loads(event["detail"])["decision"], post["id"])

    def test_plan_jobs_creates_first_reflect_after_plain_init_then_obeys_daily_cadence(self):
        job = self.con.execute("SELECT id FROM jobs WHERE project = 'proj' AND kind = 'onboard'").fetchone()
        tp.pulse.finish_job(self.con, job["id"], "human", "done")
        planned = tp.pulse.plan_jobs(self.con, "proj", now=tp.NOW)
        self.assertEqual([(j["kind"], j["source"]) for j in planned], [("reflect", "reflect:proj:2026-09-10")])
        tp.pulse.finish_job(self.con, planned[0]["id"], "human", "done")
        self.assertEqual(tp.pulse.plan_jobs(self.con, "proj", now=tp.NOW), [])

    def test_onboard_reflect_counts_as_today_but_not_tomorrow(self):
        repo = tp.make_repo(self.tmp, "cadence-app")
        code, out, err = tp.run_cli("onboard", str(repo))
        self.assertEqual(code, 0, err)
        for row in self.con.execute("SELECT id FROM jobs WHERE project = 'cadence-app'").fetchall():
            tp.pulse.finish_job(self.con, row["id"], "human", "done")
        self.assertEqual(tp.pulse.plan_jobs(self.con, "cadence-app", now=tp.pulse.utc_now()), [])
        tomorrow = (tp.pulse.parse_iso(tp.pulse.utc_now()) + tp.pulse._dt.timedelta(days=1)).strftime("%Y-%m-%dT12:00:00Z")
        planned = tp.pulse.plan_jobs(self.con, "cadence-app", now=tomorrow)
        self.assertEqual([(j["kind"], j["source"]) for j in planned],
                         [("reflect", "reflect:cadence-app:%s" % tomorrow[:10])])


if __name__ == "__main__":
    import unittest
    unittest.main()
