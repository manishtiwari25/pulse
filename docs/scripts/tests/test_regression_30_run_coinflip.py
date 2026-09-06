#!/usr/bin/env python3
"""Regression test: reproduce the 30-run coin-flip signal.

Simulate many fixtures with 30 deterministic runs each (seeded RNG) where
each run's `tier_match` is a fair coin. With 30 runs the per-fixture median
(is majority true) is approximately a coin-flip; over many fixtures the
fraction of fixtures with median True should be ~0.5. This deterministic
simulation reproduces that signal so future changes that accidentally
reduce/inflate variance are caught.
"""

import importlib.util
import random
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location(
    "evolution_measure",
    HERE.parent / "evolution-measure.py",
)
measure = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(measure)


class ThirtyRunCoinFlip(unittest.TestCase):
    def test_30_run_coinflip_fraction_close_to_half(self):
        # Deterministic seed to reproduce the signal exactly across environments
        rng = random.Random(12345)
        FIXTURES = 1000
        RUNS = 30
        records = []
        for f in range(FIXTURES):
            fid = f"F{f:04d}"
            for i in range(1, RUNS + 1):
                # Fair coin: True with p=0.5
                tier_match = rng.random() < 0.5
                records.append({
                    "label": "T",
                    "fixture": fid,
                    "run_index": i,
                    "rev": "r",
                    "model": "m",
                    "held_out": False,
                    "grade": {"tier_match": tier_match},
                    "credits": None,
                    "tokens_in": None,
                    "wall_s": None,
                    "timeout": False,
                })
        s = measure.summarize(records)
        # s1 is rounded to 2 decimals in summarize; expect it close to 0.5
        self.assertIn("s1", s)
        self.assertIsNotNone(s["s1"])
        self.assertGreaterEqual(s["s1"], 0.40)
        self.assertLessEqual(s["s1"], 0.60)


if __name__ == "__main__":
    unittest.main()
