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
if [[ "$*" == *storeTokenPlaintext* ]]; then
  cat > /dev/null
  printf 'settings=%s\n' "${PULSE_ADMIN_PREP_NOTE:-written}"
  exit "${PULSE_ADMIN_PREP_RC:-0}"
fi
cat > "$PULSE_ADMIN_STDIN"
printf '%s' "${PULSE_ADMIN_OUT:-}"
printf '%s' "${PULSE_ADMIN_ERR:-}" >&2
exit "${PULSE_ADMIN_RC:-0}"
"""


FAKE_COPILOT = r"""#!/bin/sh
printf '%%s\n' "$*" >> '%(log)s'
if [ -e '%(flag)s' ]; then
  echo 'Error: Authentication failed (Request ID: TEST) - the token may be invalid, expired, or lacking the required permissions' >&2
  echo "Run 'copilot login' to re-authenticate. If using a Fine-Grained PAT, ensure it has the 'Copilot Requests' permission" >&2
  exit 1
fi
echo ok
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
            "HOME", "PATH", "PULSE_ADMIN_LOG", "PULSE_ADMIN_STDIN", "PULSE_ADMIN_RC", "PULSE_ADMIN_OUT", "PULSE_ADMIN_ERR",
            "PULSE_ADMIN_PREP_RC", "PULSE_ADMIN_PREP_NOTE")}
        admin_home = self.tmp / "admin-home"
        admin_home.mkdir()
        os.environ["HOME"] = str(admin_home)
        os.environ["PATH"] = "%s:%s" % (shim_dir, os.environ.get("PATH", ""))
        os.environ["PULSE_ADMIN_LOG"] = str(self.tmp / "admin.log")
        os.environ["PULSE_ADMIN_STDIN"] = str(self.tmp / "admin.stdin")
        os.environ.pop("PULSE_ADMIN_RC", None)
        os.environ.pop("PULSE_ADMIN_OUT", None)
        os.environ.pop("PULSE_ADMIN_ERR", None)
        os.environ.pop("PULSE_ADMIN_PREP_RC", None)
        os.environ.pop("PULSE_ADMIN_PREP_NOTE", None)
        # The login check runs "copilot" through the hands; this stand-in answers ok, or refuses the login when the
        # flag file exists, so no test ever reaches Copilot or GitHub.
        self.refuse_flag = self.tmp / "copilot-refuses"
        self.copilot_log = self.tmp / "copilot.log"
        fake = self.tmp / "fake-copilot"
        fake.write_text(FAKE_COPILOT % {"flag": self.refuse_flag, "log": self.copilot_log}, encoding="utf-8")
        fake.chmod(0o755)
        tp.pulse.set_setting(self.con, "runner_cmd", str(fake), "human")
        self.con.commit()
        self.facts = {"ok": None, "user": "", "expires": "", "note": "GitHub not reached (test); the token's validity is unknown"}
        self._facts_patch = unittest.mock.patch.object(tp.pulse, "github_token_facts", side_effect=lambda token, **kw: dict(self.facts))
        self._facts_patch.start()
        self.add_hands("hands-2")

    def tearDown(self):
        self._facts_patch.stop()
        for key, value in self._login_env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        super().tearDown()

    def copilot_calls(self):
        return self.copilot_log.read_text(encoding="utf-8").splitlines() if self.copilot_log.exists() else []

    def check_events(self):
        return [json.loads(r["detail"]) for r in self.con.execute(
            "SELECT detail FROM events WHERE action = 'executor.login-check' AND subject = 'hands-2' ORDER BY rowid")]

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

    def admin_lines(self):
        return self.admin_log.read_text(encoding="utf-8").splitlines() if self.admin_log.exists() else []

    def run_login(self, token="", remove=False):
        if remove:
            return tp.run_cli("executor", "login", "hands-2", "--admin", "admin@box.example", "--remove")
        with unittest.mock.patch("sys.stdin", io.TextIOWrapper(io.BytesIO(token.encode("utf-8")))):
            return tp.run_cli("executor", "login", "hands-2", "--admin", "admin@box.example", "--with-token")


class ExecutorLoginTestCase(LoginFixture):
    LOGIN_COMMAND = "sudo -n -u pulse -H timeout 90 copilot login --with-token"
    REMOVE_COMMAND = "sudo -n -u pulse -H sh -c 'rm -rf -- \"$HOME/.copilot\"'"

    def test_login_command_and_prep_script_are_stable(self):
        self.assertEqual(tp.pulse.LOGIN_COMMAND, self.LOGIN_COMMAND)
        prep = tp.pulse.LOGIN_PREP_COMMAND
        self.assertTrue(prep.startswith("sudo -n -u pulse -H sh -c '") and prep.endswith("'"))
        self.assertNotIn("'", tp.pulse.LOGIN_PREP_SCRIPT)
        self.assertIn('"storeTokenPlaintext": true', tp.pulse.LOGIN_PREP_SCRIPT.replace("\\", ""))
        self.assertIn("settings.json", prep)
        self.assertNotIn("config.json", prep)
        self.assertIn("umask 077", prep)
        self.assertEqual(subprocess.run(["sh", "-n", "-c", tp.pulse.LOGIN_PREP_SCRIPT], capture_output=True).returncode, 0)

    def test_prep_script_writes_merges_and_keeps_settings_json(self):
        home = self.tmp / "prep-home"
        home.mkdir()
        env = dict(os.environ, HOME=str(home))
        run = lambda: subprocess.run(["sh", "-c", tp.pulse.LOGIN_PREP_SCRIPT], capture_output=True, text=True, env=env)
        proc = run()
        self.assertEqual((proc.returncode, proc.stdout.strip()), (0, "settings=written"), proc.stderr)
        settings = home / ".copilot" / "settings.json"
        self.assertEqual(json.loads(settings.read_text(encoding="utf-8")), {"storeTokenPlaintext": True})
        self.assertEqual(stat.S_IMODE(settings.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(settings.parent.stat().st_mode), 0o700)
        proc = run()
        self.assertEqual((proc.returncode, proc.stdout.strip()), (0, "settings=present"), proc.stderr)
        if shutil.which("node"):
            settings.write_text('{\n  "model": "x"\n}\n', encoding="utf-8")
            proc = run()
            self.assertEqual((proc.returncode, proc.stdout.strip()), (0, "settings=merged"), proc.stderr)
            self.assertEqual(json.loads(settings.read_text(encoding="utf-8")), {"model": "x", "storeTokenPlaintext": True})

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
                self.assertEqual(self.admin_lines(), [tp.pulse.LOGIN_PREP_COMMAND, self.LOGIN_COMMAND])
                self.assertEqual(self.admin_stdin.read_bytes(), token.encode("utf-8") + b"\n")
                self.assertIn("settings=written", out)
                detail = self.latest_event_detail(event_count)
                event_count += 1
                self.assertEqual(detail, {"kind": kind, "ok": True, "rc": 0})
                row = tp.pulse.executor_row(self.con, "hands-2")
                self.assertNotIn(token, json.dumps(detail))
                self.assertNotIn(token, out)
                self.assertNotIn(token, row["probe_note"])
                self.assertNotIn(token, self.admin_log.read_text(encoding="utf-8"))
                self.assertIn("b64 ", Path(os.environ["PULSE_SHIM_LOG"]).read_text(encoding="utf-8"))

    def test_prep_failure_keeps_the_token_home(self):
        token = "github_pat_" + "A" * 82
        os.environ["PULSE_ADMIN_PREP_RC"] = "4"
        os.environ["PULSE_ADMIN_PREP_NOTE"] = "unmergeable"
        code, out, err = self.run_login(token)
        self.assertEqual(code, 1)
        self.assertIn("the token was not sent", out)
        self.assertIn("settings=unmergeable", out)
        self.assertEqual(self.admin_lines(), [tp.pulse.LOGIN_PREP_COMMAND])
        self.assertFalse(self.admin_stdin.exists() and self.admin_stdin.read_bytes())
        self.assertNotIn(token, out)
        self.assertEqual(self.event_detail(), {"kind": "pat", "ok": False, "rc": 4, "step": "settings"})

    def test_login_reactivates_a_drained_executor_when_the_probe_passes(self):
        tp.pulse.set_executor_status(self.con, "hands-2", "drained", note="copilot is not logged in")
        self.con.commit()
        code, out, err = self.run_login("gho_" + "B" * 36)
        self.assertEqual(code, 0, err)
        self.assertEqual(tp.pulse.executor_row(self.con, "hands-2")["status"], "active")
        self.assertIn("hands-2: ok, active:", out)

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

    def test_logged_in_requires_a_login_record_in_config_json(self):
        copilot_home = self.home / ".copilot"
        for stage, prepare in (
            ("no folder", lambda: None),
            ("logs only", lambda: (copilot_home / "logs").mkdir(parents=True)),
            ("settings consent only", lambda: (copilot_home / "settings.json").write_text('{"storeTokenPlaintext": true}\n', encoding="utf-8")),
            ("empty config", lambda: (copilot_home / "config.json").write_text("{}\n", encoding="utf-8")),
            ("logged out", lambda: (copilot_home / "config.json").write_text(
                '// This file is managed automatically.\n{\n  "loggedInUsers": [],\n  "authTokens": {}\n}\n', encoding="utf-8")),
        ):
            with self.subTest(stage=stage):
                prepare()
                proc = self.probe()
                self.assertEqual(proc.returncode, 0, proc.stderr)
                self.assertIn("logged_in=no", proc.stdout)
        for stage, body in (
            ("plaintext store", '{\n  "authTokens": {\n    "https://github.com:someone": "not-a-real-token"\n  },\n'
                                '  "lastLoggedInUser": {"host": "https://github.com", "login": "someone"},\n'
                                '  "loggedInUsers": [{"host": "https://github.com", "login": "someone"}]\n}\n'),
            ("keychain store", '// User settings belong in settings.json.\n{\n  "loggedInUsers": [\n    {"host": "https://github.com", "login": "someone"}\n  ]\n}\n'),
        ):
            with self.subTest(stage=stage):
                (copilot_home / "config.json").write_text(body, encoding="utf-8")
                proc = self.probe()
                self.assertEqual(proc.returncode, 0, proc.stderr)
                self.assertIn("logged_in=yes", proc.stdout)
                self.assertNotIn("not-a-real-token", proc.stdout)

    def test_logged_in_accepts_a_token_from_the_environment(self):
        (self.home / ".copilot" / "logs").mkdir(parents=True)
        os.environ["COPILOT_GITHUB_TOKEN"] = "github_pat_" + "Z" * 82
        try:
            proc = self.probe()
        finally:
            os.environ.pop("COPILOT_GITHUB_TOKEN", None)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("logged_in=yes", proc.stdout)
        self.assertNotIn("Z" * 20, proc.stdout)


if __name__ == "__main__":
    unittest.main()


class LoginCheckTestCase(LoginFixture):
    """A stored login is not a working one: the login check makes one real copilot call through the hands, drains a
    machine Copilot refuses, and the daemon's recheck returns it to service when the token is fixed."""

    def row(self):
        return tp.pulse.executor_row(self.con, "hands-2")

    def test_login_runs_the_check_and_drains_when_copilot_refuses_the_token(self):
        self.refuse_flag.write_text("", encoding="utf-8")
        code, out, err = self.run_login("github_pat_" + "A" * 82)
        self.assertEqual(code, 1, out)
        self.assertIn("login check FAILED (drained)", out)
        self.assertIn("Copilot Requests permission", out)
        self.assertIn("Authentication failed", out)
        row = self.row()
        self.assertEqual(row["status"], "drained")
        self.assertTrue(tp.pulse.login_drained(row), row["probe_note"])
        self.assertEqual(row["backoff_minutes"], 15)
        self.assertTrue(row["next_probe_at"])
        self.assertEqual([e["kind"] for e in self.check_events()], ["refused"])
        self.assertEqual(self.latest_event_detail(), {"kind": "pat", "ok": False, "rc": 0})
        # one call, one-word prompt, no tools, the worker model, run inside a clone the hands then removed
        self.assertEqual(len(self.copilot_calls()), 1)
        self.assertIn("-p %s --no-ask-user --disable-builtin-mcps -s" % tp.pulse.LOGIN_CHECK_PROMPT, self.copilot_calls()[0])
        self.assertEqual([p.name for p in self.remote.iterdir() if p.name.startswith("login-check-")], [])

    def test_login_passes_the_check_and_records_what_github_said(self):
        self.facts = {"ok": True, "user": "octocat", "expires": "2027-01-01 00:00:00 UTC", "note": "GitHub accepts the token for octocat"}
        code, out, err = self.run_login("github_pat_" + "A" * 82)
        self.assertEqual(code, 0, out)
        self.assertIn("GitHub accepts the token for octocat", out)
        self.assertIn("hands-2: ok, active: login ok", out)
        self.assertEqual(self.latest_event_detail(), {"kind": "pat", "ok": True, "rc": 0, "expires": "2027-01-01 00:00:00 UTC", "user": "octocat"})
        self.assertEqual(tp.pulse.login_expiry(self.con, "hands-2"), "2027-01-01 00:00:00 UTC")
        self.assertEqual([e["kind"] for e in self.check_events()], ["ok"])
        listing = tp.pulse.executors_text(self.con)
        self.assertIn("login ok at", listing)
        self.assertIn("token expires 2027-01-01", listing)

    def test_login_warns_when_the_token_expires_soon(self):
        soon = tp.pulse.plus_hours(tp.pulse.utc_now(), 48)[:10]
        self.facts = {"ok": True, "user": "octocat", "expires": "%s 00:00:00 UTC" % soon, "note": "GitHub accepts the token for octocat"}
        code, out, err = self.run_login("github_pat_" + "A" * 82)
        self.assertEqual(code, 0, out)
        self.assertIn("warning: the token expires in 2 day(s), on %s" % soon, out)
        self.assertIn("WARNING the token expires", tp.pulse.executors_text(self.con))

    def test_login_refuses_a_token_github_rejects_without_sending_it(self):
        self.facts = {"ok": False, "user": "", "expires": "", "note": "GitHub refuses the token (401): invalid, expired, or revoked - the token was not sent"}
        code, out, err = self.run_login("github_pat_" + "A" * 82)
        self.assertEqual(code, 1)
        self.assertIn("GitHub refuses the token (401)", out)
        self.assertEqual(self.admin_lines(), [], "nothing went to the machine")
        self.assertEqual(self.event_detail(), {"kind": "pat", "ok": False, "rc": 1, "step": "github"})
        self.assertEqual(self.copilot_calls(), [])

    def test_a_shallow_probe_keeps_a_login_drained_machine_drained_and_the_deep_probe_returns_it(self):
        self.refuse_flag.write_text("", encoding="utf-8")
        self.run_login("gho_" + "B" * 36)
        self.assertEqual(self.row()["status"], "drained")
        ok, note = tp.pulse.probe_executor(self.con, "hands-2", actor=tp.pulse.HUMAN_AGENT, now=tp.NOW)
        self.assertTrue(ok, note)
        row = self.row()
        self.assertEqual(row["status"], "drained", "logged_in=yes is not a working login")
        self.assertTrue(tp.pulse.login_drained(row), row["probe_note"])
        # `pulse executor probe` of such a machine includes the login check without --login
        code, out, err = tp.run_cli("executor", "probe", "hands-2")
        self.assertEqual(code, 1, out)
        self.assertIn("login check FAILED (drained)", out)
        self.refuse_flag.unlink()
        code, out, err = tp.run_cli("executor", "probe", "hands-2", "--login")
        self.assertEqual(code, 0, out)
        self.assertIn("login check ok (active)", out)
        row = self.row()
        self.assertEqual((row["status"], row["backoff_minutes"], row["next_probe_at"]), ("active", 0, ""))
        self.assertTrue(self.con.execute("SELECT 1 FROM events WHERE action = 'executor.recovered' AND subject = 'hands-2'").fetchone())

    def test_the_daemon_rechecks_with_a_doubling_wait_and_recovers_by_itself(self):
        self.refuse_flag.write_text("", encoding="utf-8")
        self.run_login("gho_" + "B" * 36)
        row = self.row()
        self.assertEqual((row["status"], row["backoff_minutes"]), ("drained", 15))
        first_due = row["next_probe_at"]
        # not due yet: nothing happens
        self.assertEqual(tp.pulse.reprobe_due(self.con, interval=300, now=tp.pulse.plus_hours(first_due, -1 / 60.0)), [])
        self.assertEqual(len(self.check_events()), 1)
        # due, still refused: waits twice as long
        self.assertEqual(tp.pulse.reprobe_due(self.con, interval=300, now=first_due), [])
        row = self.row()
        self.assertEqual((row["status"], row["backoff_minutes"]), ("drained", 30))
        self.assertGreater(row["next_probe_at"], first_due)
        self.assertEqual([e["kind"] for e in self.check_events()], ["refused", "refused"])
        # the human fixed the token on GitHub: the next recheck brings the machine back
        self.refuse_flag.unlink()
        back = tp.pulse.reprobe_due(self.con, interval=300, now=row["next_probe_at"])
        self.assertEqual([name for name, _ in back], ["hands-2"])
        row = self.row()
        self.assertEqual((row["status"], row["backoff_minutes"], row["next_probe_at"]), ("active", 0, ""))
        self.assertEqual([e["kind"] for e in self.check_events()], ["refused", "refused", "ok"])

    def test_the_wait_never_exceeds_six_hours(self):
        self.refuse_flag.write_text("", encoding="utf-8")
        self.run_login("gho_" + "B" * 36)
        self.con.execute("UPDATE executors SET backoff_minutes = 300, next_probe_at = ? WHERE name = 'hands-2'", (tp.NOW,))
        self.con.commit()
        tp.pulse.reprobe_due(self.con, interval=300, now=tp.NOW)
        self.assertEqual(self.row()["backoff_minutes"], 360)

    def test_a_launch_fault_schedules_the_first_recheck(self):
        tp.pulse.note_executor_run(self.con, "hands-2", "error", note="worker rejected at launch (exit 1 after 8s): Authentication failed",
                                   now=tp.NOW, fault=True)
        row = self.row()
        self.assertEqual((row["status"], row["backoff_minutes"]), ("drained", 15))
        self.assertTrue(tp.pulse.login_drained(row))
        # a fault that is not about the login drains without a recheck (a human probe returns it, as before)
        tp.pulse.set_executor_status(self.con, "hands-2", "active", note="ok")
        tp.pulse.note_executor_run(self.con, "hands-2", "error", note="hands predate ADR-022", now=tp.NOW, fault=True)
        row = self.row()
        self.assertEqual((row["status"], row["backoff_minutes"], row["next_probe_at"]), ("drained", 0, ""))
        self.assertFalse(tp.pulse.login_drained(row))

    def test_expiry_warning_and_fault_line(self):
        self.assertEqual(tp.pulse.expiry_warning("", now=tp.NOW), "")
        self.assertEqual(tp.pulse.expiry_warning("2030-01-01 00:00:00 UTC", now=tp.NOW), "")
        self.assertIn("expired on 2020-01-01", tp.pulse.expiry_warning("2020-01-01 12:00:00 UTC", now=tp.NOW))
        self.assertIn("expires in 3 day(s)", tp.pulse.expiry_warning(tp.pulse.plus_hours(tp.NOW, 72)[:10], now=tp.NOW))
        self.assertEqual(tp.pulse.fault_line("warming up\nError: Authentication failed (Request ID: X)\n • hint"), "Error: Authentication failed (Request ID: X)")
        self.assertEqual(tp.pulse.fault_line("\n\nok\n"), "ok")
        self.assertEqual(tp.pulse.fault_line(""), "")


class GithubTokenFactsTestCase(unittest.TestCase):
    """The token goes to GitHub once, before it goes anywhere else; what GitHub says decides whether it is sent."""

    class _Resp:
        def __init__(self, body, headers):
            self._body = body
            self.headers = headers

        def read(self):
            return self._body

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

    def test_accepted_token_reports_user_and_expiry(self):
        seen = {}

        def fake_urlopen(req, timeout=0):
            seen["url"] = req.full_url
            seen["auth"] = req.get_header("Authorization")
            return self._Resp(b'{"login": "octocat"}', {"github-authentication-token-expiration": "2027-01-01 00:00:00 UTC"})

        with unittest.mock.patch.object(tp.pulse.urllib.request, "urlopen", fake_urlopen):
            facts = tp.pulse.github_token_facts("github_pat_" + "A" * 82, api="https://api.example")
        self.assertEqual(seen["url"], "https://api.example/user")
        self.assertEqual(seen["auth"], "Bearer github_pat_" + "A" * 82)
        self.assertEqual((facts["ok"], facts["user"], facts["expires"]), (True, "octocat", "2027-01-01 00:00:00 UTC"))
        self.assertIn("expires 2027-01-01", facts["note"])

    def test_refused_and_unreachable(self):
        def refused(req, timeout=0):
            raise tp.pulse.urllib.error.HTTPError(req.full_url, 401, "Unauthorized", {}, None)

        def gone(req, timeout=0):
            raise tp.pulse.urllib.error.URLError("no route")

        with unittest.mock.patch.object(tp.pulse.urllib.request, "urlopen", refused):
            facts = tp.pulse.github_token_facts("gho_" + "B" * 36, api="https://api.example")
        self.assertIs(facts["ok"], False)
        self.assertIn("not sent", facts["note"])
        with unittest.mock.patch.object(tp.pulse.urllib.request, "urlopen", gone):
            facts = tp.pulse.github_token_facts("gho_" + "B" * 36, api="https://api.example")
        self.assertIsNone(facts["ok"])
        self.assertIn("not reached", facts["note"])
