#!/usr/bin/env python3
"""Tests for the owner-run evolution setup helper (ADR-012, WU-003)."""

import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "evolution-setup.sh"

CONSTITUTION = """---
loop: off
limits: none
---

# Constitution
"""

EVALUATORS = """---
baseline: B-001
---

# Evaluators

## Held-out set

Held out: pending
"""

ARCHIVE = """# Archive

## Baselines

| Row | Date | Tree | Constraints | S1 | Notes |
| --- | --- | --- | --- | --- | --- |
| B-001 | 2026-09-04 | main | pass | 1.0 | fixture |
"""


class EvolutionSetupTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="pulse-evolution-setup-")
        self.root = Path(self.tmp.name)
        self.repo = self.root / "repo"
        self.store = self.root / "private-held-out"
        evolution = self.repo / "docs" / "evolution"
        evolution.mkdir(parents=True)
        (evolution / "constitution.md").write_text(CONSTITUTION, encoding="utf-8")
        (evolution / "evaluators.md").write_text(EVALUATORS, encoding="utf-8")
        (evolution / "archive.md").write_text(ARCHIVE, encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    @property
    def constitution(self):
        return self.repo / "docs" / "evolution" / "constitution.md"

    @property
    def evaluators(self):
        return self.repo / "docs" / "evolution" / "evaluators.md"

    @property
    def archive(self):
        return self.repo / "docs" / "evolution" / "archive.md"

    def run_helper(self, command, store=None):
        args = ["bash", str(SCRIPT), command, "--repo", str(self.repo)]
        if store is not None:
            args += ["--store", str(store)]
        return subprocess.run(args, capture_output=True, text=True, check=False)

    def snapshot_gates(self):
        return (self.constitution.read_bytes(), self.evaluators.read_bytes())

    def prepare(self):
        result = self.run_helper("prepare", self.store)
        self.assertEqual(result.returncode, 0, result.stderr)

    def fill_fixture(self, fixture_id, secret):
        path = self.store / f"{fixture_id}.md"
        path.write_text(
            "\n".join(
                [
                    f"# Private Held-Out Fixture {fixture_id}",
                    "",
                    "tier: 1",
                    f"request: {secret} request",
                    f"expected: {secret} expected result",
                    f"checks: {secret} acceptance check",
                    "",
                ]
            ),
            encoding="utf-8",
        )
        path.chmod(0o600)

    def fill_valid_store(self):
        self.fill_fixture("H01", "SECRET-H01")
        self.fill_fixture("H02", "SECRET-H02")

    def test_status_names_ids_and_exact_next_command_without_writing(self):
        before = self.snapshot_gates()
        result = self.run_helper("status")

        self.assertEqual(result.returncode, 1)
        self.assertIn("helper ids:  H01, H02", result.stdout)
        self.assertIn("evolution-setup.sh prepare --store /private/path", result.stdout)
        self.assertEqual(self.snapshot_gates(), before)
        self.assertFalse(self.store.exists())

    def test_prepare_creates_private_templates_and_preserves_existing_file(self):
        self.store.mkdir(mode=0o700)
        existing = self.store / "H01.md"
        existing.write_text("owner content\n", encoding="utf-8")
        existing.chmod(0o600)

        result = self.run_helper("prepare", self.store)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(existing.read_text(encoding="utf-8"), "owner content\n")
        self.assertTrue((self.store / "H02.md").is_file())
        self.assertEqual(stat.S_IMODE((self.store / "H02.md").stat().st_mode), 0o600)

    def age_store(self, hours=1):
        """Date both fixtures before the pending decision (evaluators.md's time)."""
        import os

        past = self.evaluators.stat().st_mtime - hours * 3600
        for fixture_id in ("H01", "H02"):
            os.utime(self.store / f"{fixture_id}.md", (past, past))

    def test_pending_set_treats_fixtures_older_than_the_decision_as_stale(self):
        # Incident I-001: the set went pending because the fixtures leaked.
        # Files written before that decision must never be re-armed.
        self.store.mkdir(mode=0o700)
        self.fill_valid_store()
        self.age_store()
        before = self.snapshot_gates()

        status = self.run_helper("status", self.store)
        self.assertEqual(status.returncode, 1)
        self.assertIn("private set: stale - H01 H02 predate the pending decision", status.stdout)
        self.assertIn("evolution-setup.sh prepare --store", status.stdout)
        self.assertNotIn("activate --store", status.stdout)

        activate = self.run_helper("activate", self.store)
        self.assertEqual(activate.returncode, 1)
        self.assertIn("predate the pending decision", activate.stderr)
        self.assertEqual(self.snapshot_gates(), before)

        prepare = self.run_helper("prepare", self.store)
        self.assertEqual(prepare.returncode, 0, prepare.stderr)
        retired = sorted(self.store.glob("H0?.retired-*.md"))
        self.assertEqual(len(retired), 2)
        self.assertIn("SECRET-H01", retired[0].read_text(encoding="utf-8"))
        self.assertEqual(stat.S_IMODE(retired[0].stat().st_mode), 0o600)
        self.assertIn("tier: TODO", (self.store / "H01.md").read_text(encoding="utf-8"))
        self.assertNotIn("SECRET", prepare.stdout)

        self.fill_valid_store()
        activated = self.run_helper("activate", self.store)
        self.assertEqual(activated.returncode, 0, activated.stderr)
        self.assertIn("Held out: H01, H02", self.evaluators.read_text(encoding="utf-8"))

    def test_active_set_never_calls_old_fixtures_stale(self):
        self.evaluators.write_text(
            EVALUATORS.replace("Held out: pending", "Held out: H01, H02"), encoding="utf-8"
        )
        self.constitution.write_text(CONSTITUTION.replace("loop: off", "loop: on"), encoding="utf-8")
        self.store.mkdir(mode=0o700)
        self.fill_valid_store()
        self.age_store()

        status = self.run_helper("status", self.store)
        self.assertEqual(status.returncode, 0, status.stdout)
        self.assertIn("private set: ready", status.stdout)

    def test_prepare_refuses_store_inside_repository_before_creation(self):
        inside = self.repo / "private" / "held-out"
        result = self.run_helper("prepare", inside)

        self.assertEqual(result.returncode, 2)
        self.assertIn("inside the repository", result.stderr)
        self.assertFalse((self.repo / "private").exists())

    def test_prepare_refuses_symlink_resolving_inside_repository(self):
        inside = self.repo / "private"
        inside.mkdir()
        linked_store = self.root / "linked-private"
        linked_store.symlink_to(inside, target_is_directory=True)

        result = self.run_helper("prepare", linked_store)

        self.assertEqual(result.returncode, 2)
        self.assertIn("inside the repository", result.stderr)
        self.assertFalse((inside / "H01.md").exists())
        self.assertFalse((inside / "H02.md").exists())

    def test_activate_rejects_unfilled_fixture_without_changing_gates(self):
        self.prepare()
        before = self.snapshot_gates()

        result = self.run_helper("activate", self.store)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is not filled", result.stderr)
        self.assertEqual(self.snapshot_gates(), before)

    def test_activate_updates_only_gates_without_printing_fixture_contents(self):
        self.prepare()
        self.fill_valid_store()
        before_constitution = self.constitution.read_text(encoding="utf-8")
        before_evaluators = self.evaluators.read_text(encoding="utf-8")

        result = self.run_helper("activate", self.store)

        self.assertEqual(result.returncode, 0, result.stderr)
        output = result.stdout + result.stderr
        self.assertNotIn("SECRET-H01", output)
        self.assertNotIn("SECRET-H02", output)
        self.assertEqual(
            self.constitution.read_text(encoding="utf-8"),
            before_constitution.replace("loop: off", "loop: on"),
        )
        self.assertEqual(
            self.evaluators.read_text(encoding="utf-8"),
            before_evaluators.replace("Held out: pending", "Held out: H01, H02"),
        )

        after = self.snapshot_gates()
        repeated = self.run_helper("activate", self.store)
        self.assertEqual(repeated.returncode, 0, repeated.stderr)
        self.assertEqual(self.snapshot_gates(), after)

    def test_status_accepts_existing_non_helper_held_out_ids(self):
        self.constitution.write_text(CONSTITUTION.replace("loop: off", "loop: on"), encoding="utf-8")
        self.evaluators.write_text(
            EVALUATORS.replace("Held out: pending", "Held out: PRIVATE-A, PRIVATE-B"),
            encoding="utf-8",
        )

        result = self.run_helper("status")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Ready: repository evolution gates are active.", result.stdout)

    def test_activate_preserves_existing_non_helper_held_out_ids(self):
        self.prepare()
        self.fill_valid_store()
        self.evaluators.write_text(
            EVALUATORS.replace("Held out: pending", "Held out: PRIVATE-A, PRIVATE-B"),
            encoding="utf-8",
        )
        before = self.snapshot_gates()

        result = self.run_helper("activate", self.store)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already has a non-pending held-out set", result.stderr)
        self.assertEqual(self.snapshot_gates(), before)

    def test_activate_refuses_stop_or_missing_baseline_without_changes(self):
        self.prepare()
        self.fill_valid_store()

        stop = self.repo / "docs" / "evolution" / "STOP"
        stop.touch()
        before = self.snapshot_gates()
        stopped = self.run_helper("activate", self.store)
        self.assertNotEqual(stopped.returncode, 0)
        self.assertIn("STOP is present", stopped.stderr)
        self.assertEqual(self.snapshot_gates(), before)
        stop.unlink()

        self.archive.write_text("# Archive without the baseline row\n", encoding="utf-8")
        missing = self.run_helper("activate", self.store)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("is not recorded", missing.stderr)
        self.assertEqual(self.snapshot_gates(), before)

    def test_activate_refuses_unwritable_or_symlinked_gate_without_changes(self):
        self.prepare()
        self.fill_valid_store()
        original = self.constitution.read_text(encoding="utf-8")
        real_constitution = self.constitution.with_name("constitution.real.md")
        self.constitution.rename(real_constitution)
        self.constitution.symlink_to(real_constitution.name)
        before = self.snapshot_gates()

        result = self.run_helper("activate", self.store)

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be a regular file", result.stderr)
        self.assertEqual(self.snapshot_gates(), before)
        self.constitution.unlink()
        real_constitution.rename(self.constitution)
        self.assertEqual(self.constitution.read_text(encoding="utf-8"), original)

    def test_activate_refuses_loose_permissions_or_duplicate_gate(self):
        self.prepare()
        self.fill_valid_store()
        (self.store / "H01.md").chmod(0o644)
        before = self.snapshot_gates()

        permissions = self.run_helper("activate", self.store)
        self.assertNotEqual(permissions.returncode, 0)
        self.assertIn("permissions", permissions.stderr)
        self.assertEqual(self.snapshot_gates(), before)

        (self.store / "H01.md").chmod(0o600)
        self.evaluators.write_text(EVALUATORS + "\nHeld out: duplicate\n", encoding="utf-8")
        before_duplicate = self.snapshot_gates()
        duplicate = self.run_helper("activate", self.store)
        self.assertNotEqual(duplicate.returncode, 0)
        self.assertIn("exactly one 'Held out:'", duplicate.stderr)
        self.assertEqual(self.snapshot_gates(), before_duplicate)


if __name__ == "__main__":
    unittest.main()
