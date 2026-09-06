#!/usr/bin/env python3
"""Tests for ADR-021 Amendment 1: token login for remote hands."""

import io
import json
import os
import base64
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import test_pulse as tp


ADMIN_SSH_SHIM = r"""#!/bin/bash
while [ $# -gt 0 ]; do
  case "$1" in
    -o|-p|-i|-l|-F) shift 2 ;;
    -*) shift ;;
    *) break ;;
  esac
done
host="$1"; shift
if [[ "${1:-}" == b64\ * ]]; then
  echo "$host $*" >> "$PULSE_SHIM_LOG"
  export SSH_ORIGINAL_COMMAND="$*"
  exec bash "$PULSE_HANDS"
fi
if [ "${PULSE_ADMIN_RC:-0}" = "255" ]; then
  echo "ssh: connect to host $host port 22: Connection refused" >&2
  exit 255
fi
printf '%s\n' "$*" >> "$PULSE_ADMIN_LOG"
cat > "$PULSE_ADMIN_STDIN"
printf '%s' "${PULSE_ADMIN_OUT:-}"
printf '%s' "${PULSE_ADMIN_ERR:-}" >&2
exit "${PULSE_ADMIN_RC:-0}"
"""


class LoginFixture(tp.HandsFixture):
    def setUp(self):
        super().setUp()
        shim_dir = self.tmp / "admin-bin"
        shim_dir.mkdir()
        shim = shim_dir / "ssh"
        shim.write_text(ADMIN_SSH_SHIM, encoding="utf-8")
        shim.chmod(0o755)
        self._login_env_before = {k: os.environ.get(k) for k in (
            "HOME", "PATH", "PULSE_ADMIN_LOG", "PULSE_ADMIN_STDIN", "PULSE_ADMIN_RC", "PULSE_ADMIN_OUT", "PULSE_ADMIN_ERR")}
        admin_home = self.tmp / "admin-home"
        admin_home.mkdir()
        os.environ["HOME"] = str(admin_home)
        os.environ["PATH"] = "%s:%s" % (shim_dir, os.environ.get("PATH", ""))
        os.environ["PULSE_ADMIN_LOG"] = str(self.tmp / "admin.log")
        os.environ["PULSE_ADMIN_STDIN"] = str(self.tmp / "admin.stdin")
        os.environ.pop("PULSE_ADMIN_RC", None)
        os.environ.pop("PULSE_ADMIN_OUT", None)
        os.environ.pop("PULSE_ADMIN_ERR", None)
        self.add_hands("hands-2")

    def tearDown(self):
        for key, value in self._login_env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        super().tearDown()

    @property
    def admin_log(self):
        return Path(os.environ["PULSE_ADMIN_LOG"])

    @property
    def admin_stdin(self):
        return Path(os.environ["PULSE_ADMIN_STDIN"])

    def events(self):
        return [dict(r) for r in self.con.execute("SELECT * FROM events WHERE action = 'executor.login' ORDER BY rowid")]

    def event_detail(self):
        events = self.events()
        self.assertEqual(len(events), 1)
        return json.loads(events[0]["detail"])

    def latest_event_detail(self, before=0):
        events = self.events()
        self.assertEqual(len(events), before + 1)
        return json.loads(events[-1]["detail"])

    def assert_nothing_sent(self):
        self.assertFalse(self.admin_log.exists() and self.admin_log.read_text(encoding="utf-8"))
        self.assertFalse(self.admin_stdin.exists() and self.admin_stdin.read_bytes())
        self.assertEqual(self.events(), [])

    def run_login(self, token="", remove=False):
        if remove:
            return tp.run_cli("executor", "login", "hands-2", "--admin", "admin@box.example", "--remove")
        with unittest.mock.patch("sys.stdin", io.TextIOWrapper(io.BytesIO(token.encode("utf-8")))):
            return tp.run_cli("executor", "login", "hands-2", "--admin", "admin@box.example", "--with-token")


class ExecutorLoginTestCase(LoginFixture):
    LOGIN_COMMAND = "sudo -n -u pulse -H timeout 90 copilot login --with-token"
    REMOVE_COMMAND = "sudo -n -u pulse -H sh -c 'rm -rf -- \"$HOME/.copilot\"'"

    def test_refuses_empty_stdin_without_sending_or_event(self):
        code, out, err = self.run_login("")
        self.assertEqual(code, 2)
        self.assertIn("no token on standard input", err)
        self.assert_nothing_sent()

    def test_refuses_inner_whitespace_without_sending_or_event(self):
        code, out, err = self.run_login("gho_BBBB BBBB")
        self.assertEqual(code, 2)
        self.assertIn("whitespace", err)
        self.assert_nothing_sent()

    def test_refuses_classic_pat_without_sending_or_event(self):
        code, out, err = self.run_login("ghp_" + "D" * 36)
        self.assertEqual(code, 2)
        self.assertIn("classic personal access token", err)
        self.assert_nothing_sent()

    def test_refuses_unknown_prefix_without_sending_or_event(self):
        code, out, err = self.run_login("bad_" + "E" * 36)
        self.assertEqual(code, 2)
        self.assertIn("unsupported token prefix", err)
        self.assert_nothing_sent()

    def test_accepts_supported_token_prefixes_and_never_records_token(self):
        cases = (
            ("github_pat_" + "A" * 82, "pat"),
            ("gho_" + "B" * 36, "oauth"),
            ("ghu_" + "C" * 36, "user"),
        )
        event_count = 0
        for token, kind in cases:
            with self.subTest(kind=kind):
                self.admin_log.write_text("", encoding="utf-8")
                self.admin_stdin.write_bytes(b"")
                Path(os.environ["PULSE_SHIM_LOG"]).write_text("", encoding="utf-8")
                code, out, err = self.run_login(token + "\n")
                self.assertEqual(code, 0, err)
                self.assertEqual(self.admin_log.read_text(encoding="utf-8").strip(), self.LOGIN_COMMAND)
                self.assertEqual(self.admin_stdin.read_bytes(), token.encode("utf-8") + b"\n")
                detail = self.latest_event_detail(event_count)
                event_count += 1
                self.assertEqual(detail, {"kind": kind, "ok": True, "rc": 0})
                row = tp.pulse.executor_row(self.con, "hands-2")
                self.assertNotIn(token, json.dumps(detail))
                self.assertNotIn(token, out)
                self.assertNotIn(token, row["probe_note"])
                self.assertNotIn(token, self.admin_log.read_text(encoding="utf-8"))
                self.assertIn("b64 ", Path(os.environ["PULSE_SHIM_LOG"]).read_text(encoding="utf-8"))

    def test_remote_failure_redacts_token_in_output_and_event(self):
        token = "gho_" + "B" * 36
        os.environ["PULSE_ADMIN_RC"] = "1"
        os.environ["PULSE_ADMIN_ERR"] = "Login failed: %s 401 Unauthorized\n" % token
        code, out, err = self.run_login(token)
        self.assertEqual(code, 1)
        self.assertIn("[redacted]", out)
        self.assertIn("401", out)
        self.assertNotIn(token, out)
        detail = self.event_detail()
        self.assertEqual(detail, {"kind": "oauth", "ok": False, "rc": 1})
        self.assertNotIn(token, json.dumps(detail))

    def test_unreachable_admin_records_exit_255(self):
        os.environ["PULSE_ADMIN_RC"] = "255"
        code, out, err = self.run_login("ghu_" + "C" * 36)
        self.assertEqual(code, 1)
        self.assertIn("unreachable", out)
        self.assertEqual(self.event_detail(), {"kind": "user", "ok": False, "rc": 255})

    def test_remove_deletes_login_probes_and_drains_executor(self):
        os.environ["PULSE_HANDS_ALLOW_ANY_WORKER"] = "0"
        code, out, err = self.run_login(remove=True)
        self.assertEqual(code, 0, err)
        self.assertEqual(self.admin_log.read_text(encoding="utf-8").strip(), self.REMOVE_COMMAND)
        self.assertEqual(self.event_detail(), {"kind": "removed", "ok": True, "rc": 0})
        self.assertEqual(tp.pulse.executor_row(self.con, "hands-2")["status"], "drained")

    def test_with_token_and_remove_together_refuses(self):
        code, out, err = tp.run_cli("executor", "login", "hands-2", "--admin", "admin@box.example", "--with-token", "--remove")
        self.assertEqual(code, 2)
        self.assertIn("exactly one", err)

    def test_unknown_executor_refuses(self):
        code, out, err = tp.run_cli("executor", "login", "missing", "--admin", "admin@box.example", "--remove")
        self.assertEqual(code, 2)
        self.assertIn("No executor missing", err)

    def test_cli_uses_sys_stdin_buffer_for_token(self):
        token = "gho_" + "B" * 36
        with unittest.mock.patch("sys.stdin", io.TextIOWrapper(io.BytesIO((token + "\n").encode("utf-8")))):
            code, out, err = tp.run_cli("executor", "login", "hands-2", "--admin", "admin@box.example", "--with-token")
        self.assertEqual(code, 0, err)
        self.assertEqual(self.admin_stdin.read_bytes(), token.encode("utf-8") + b"\n")


class HandsProbeLoginTestCase(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="pulse-hands-login-")).resolve()
        self.home = self.tmp / "home"
        self.home.mkdir()
        self.workdir = self.tmp / "work"
        self.workdir.mkdir()
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        copilot = self.bin / "copilot"
        copilot.write_text("#!/bin/sh\necho 'GitHub Copilot CLI 1.0.83'\n", encoding="utf-8")
        copilot.chmod(copilot.stat().st_mode | stat.S_IEXEC)
        self._env_before = {k: os.environ.get(k) for k in (
            "HOME", "PATH", "SSH_ORIGINAL_COMMAND", "PULSE_HANDS_WORKDIR", "PULSE_HANDS_ALLOW_ANY_WORKER")}
        os.environ["HOME"] = str(self.home)
        os.environ["PATH"] = "%s:%s" % (self.bin, os.environ.get("PATH", ""))
        os.environ["SSH_ORIGINAL_COMMAND"] = "b64 " + base64.b64encode(b"probe\0").decode("ascii")
        os.environ["PULSE_HANDS_WORKDIR"] = str(self.workdir)
        os.environ["PULSE_HANDS_ALLOW_ANY_WORKER"] = "0"

    def tearDown(self):
        for key, value in self._env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(self.tmp, ignore_errors=True)

    def probe(self):
        return subprocess.run(["bash", str(tp.HANDS_SH)], capture_output=True, text=True, env=os.environ)

    def test_logged_in_requires_regular_file_under_copilot_home(self):
        proc = self.probe()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("logged_in=no", proc.stdout)
        (self.home / ".copilot" / "logs").mkdir(parents=True)
        proc = self.probe()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("logged_in=no", proc.stdout)
        (self.home / ".copilot" / "config.json").write_text("{}", encoding="utf-8")
        proc = self.probe()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("logged_in=yes", proc.stdout)


if __name__ == "__main__":
    unittest.main()
