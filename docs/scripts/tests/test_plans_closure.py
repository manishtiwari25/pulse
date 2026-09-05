#!/usr/bin/env python3
"""Automated checks for plan closure verification.

These tests implement the "On closure, `delivered:` is set here and the
decision's `delivery:` mark is updated" verification as assertions so CI
catches missing closure metadata.
"""

import re
import unittest
from pathlib import Path

RE_FRONT = re.compile(r"^---\s*(.*?)\s*---\s*", re.DOTALL | re.MULTILINE)
RE_KEY = re.compile(r"^\s*([A-Za-z0-9_\-]+)\s*:\s*(.*)$", re.MULTILINE)


def parse_frontmatter(path: Path):
    text = path.read_text(encoding="utf-8")
    m = RE_FRONT.match(text)
    if not m:
        return {}
    body = m.group(1)
    data = {}
    for km in RE_KEY.finditer(body):
        key = km.group(1).strip()
        val = km.group(2).strip()
        data[key] = val
    return data


class PlansClosureTests(unittest.TestCase):
    def setUp(self):
        # repository root is three parents up from this tests file
        self.repo = Path(__file__).resolve().parents[3]
        self.plans_dir = self.repo / "docs" / "plans"
        self.decisions_dir = self.repo / "docs" / "decisions"

    def test_delivered_mark_present_when_plan_status_delivered(self):
        """If a plan is marked Delivered its frontmatter must include a non-empty delivered: value.

        Additionally, if the plan references an ADR decision, that decision's
        frontmatter must have delivery: delivered as well.
        """
        plans = sorted(self.plans_dir.glob("*.md"))
        self.assertTrue(plans, "no plan files found in docs/plans")
        adr_link_re = re.compile(r"\[ADR-[0-9A-Za-z_\-]+\]\((../decisions/[^)]+)\)")

        for plan in plans:
            with self.subTest(plan=plan.name):
                fm = parse_frontmatter(plan)
                status = fm.get("status", "").strip()

                # Only assert delivered presence for plans already marked Delivered
                if status.lower() == "delivered":
                    delivered_val = fm.get("delivered", "").strip()
                    self.assertTrue(
                        delivered_val,
                        f"Plan {plan.name} has status Delivered but no delivered: frontmatter value",
                    )

                    # If plan references an ADR decision, check that decision's delivery
                    body = plan.read_text(encoding="utf-8")
                    m = adr_link_re.search(body)
                    if m:
                        rel = m.group(1)
                        # resolve relative to the plan file's parent
                        dec_path = (plan.parent / rel).resolve()
                        if dec_path.exists():
                            d_fm = parse_frontmatter(dec_path)
                            delivery = d_fm.get("delivery", "").strip().lower()
                            self.assertEqual(
                                delivery,
                                "delivered",
                                f"Decision {dec_path.name} referenced by {plan.name} must have delivery: delivered when the plan is delivered",
                            )


if __name__ == "__main__":
    unittest.main()
