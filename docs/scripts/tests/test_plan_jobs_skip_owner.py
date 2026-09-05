import unittest
import importlib.util
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent

# load pulse module from scripts
spec = importlib.util.spec_from_file_location("pulse", SCRIPTS / "../pulse.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
pulse = module

class PlanJobsOwnerSkipTest(unittest.TestCase):
    def setUp(self):
        # reuse existing fixtures from test_pulse by creating a minimal store
        self.tmp = Path("/tmp")

    def test_is_owner_addressed_post(self):
        p = {"thread": "owner-questions", "summary": "Evaluator proposal for the owner: something"}
        self.assertTrue(pulse.is_owner_addressed_post(p))
        p = {"thread": "evolution/speed", "summary": "Release candidate f2ae14a7 - Vote requested"}
        self.assertTrue(pulse.is_owner_addressed_post(p))
        p = {"thread": "evolution/speed", "summary": "Implement caching"}
        self.assertFalse(pulse.is_owner_addressed_post(p))

if __name__ == '__main__':
    unittest.main()
