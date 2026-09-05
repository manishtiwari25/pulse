"""Every script under docs/scripts/ at least parses.

The runner verifies a landed branch with the project's own check, and for a
PULSE repository that check is this suite - so the suite has to read the
scripts too. A runner-landed reflect.sh once failed `bash -n` (2026-09-05)
and landed anyway because nothing here parsed it.
"""
import pathlib
import shutil
import subprocess
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parents[1]


class ScriptsParse(unittest.TestCase):
    @unittest.skipUnless(shutil.which("bash"), "bash is not installed")
    def test_every_shell_script_parses(self):
        scripts = sorted(SCRIPTS.glob("*.sh"))
        self.assertTrue(scripts, "no shell scripts found under %s" % SCRIPTS)
        for script in scripts:
            with self.subTest(script=script.name):
                proc = subprocess.run(["bash", "-n", str(script)], capture_output=True, text=True)
                self.assertEqual(proc.returncode, 0, proc.stderr.strip())

    def test_every_python_script_compiles(self):
        modules = sorted(SCRIPTS.glob("*.py"))
        self.assertTrue(modules, "no Python scripts found under %s" % SCRIPTS)
        for module in modules:
            with self.subTest(module=module.name):
                compile(module.read_text(encoding="utf-8"), str(module), "exec")
