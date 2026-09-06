#!/usr/bin/env python3
"""Tests for `pulse pair` (ADR-021).

Every test uses the StoreFixture PULSE_HOME and a temporary HOME. The ssh shim
never leaves the machine: executor probes go to pulse-hands.sh, while admin
calls are answered by a small handler script.
"""

import hashlib
import json
import os
import shutil
import stat
import subprocess
import tempfile
import textwrap
from pathlib import Path

import test_pulse as tp

NOW = "2026-09-10T12:00:00Z"
APPLY_SCRIPT_SHA256 = "c511f242366c42cfbedaf7456ce765f7aea33fb7a451fa1c45fbdd072040a403"


PAIR_SSH_SHIM = r"""#!/bin/bash
set -u
printf '%s\n' "$*" >> "$PULSE_ADMIN_LOG"
raw="$*"
args=("$@")
i=0
while [ "$i" -lt "${#args[@]}" ]; do
  arg="${args[$i]}"
  case "$arg" in
    -o|-p|-i|-l|-F) i=$((i + 2)) ;;
    -O) i=$((i + 2)) ;;
    -*) i=$((i + 1)) ;;
    *) break ;;
  esac
done
if [[ "$raw" == *" -O exit "* || "$raw" == "-O exit "* ]]; then
  exit 0
fi
host="${args[$i]:-}"
i=$((i + 1))
remote="${args[$i]:-}"
if [[ "$remote" == b64\ * ]]; then
  if [ "${PULSE_SHIM_FAIL:-}" = "always" ]; then
    echo "ssh: connect to host $host port 22: Connection refused" >&2
    exit 255
  fi
  export SSH_ORIGINAL_COMMAND="$remote"
  exec bash "$PULSE_HANDS"
fi
mkdir -p "$PULSE_ADMIN_STDIN_DIR"
n=$(find "$PULSE_ADMIN_STDIN_DIR" -type f -name '*.sh' | wc -l | tr -d ' ')
n=$((n + 1))
stdin="$PULSE_ADMIN_STDIN_DIR/$n.sh"
cat > "$stdin"
export ADMIN_RAW="$raw"
export ADMIN_HOST="$host"
export ADMIN_COMMAND="$remote"
export ADMIN_STDIN="$stdin"
exec bash "$PULSE_ADMIN_HANDLER"
"""


PAIR_ADMIN_HANDLER = r"""#!/bin/bash
set -u
if [ "$ADMIN_COMMAND" = "true" ]; then
  if [[ "$ADMIN_RAW" == *"ControlMaster=yes"* ]]; then
    exit 0
  fi
  exit "${PAIR_FAKE_RECHECK_RC:-0}"
fi
if grep -q '/usr/local/bin/pulse-hands.tmp' "$ADMIN_STDIN"; then
  echo "step=user result=done"
  echo "step=hands result=done"
  echo "step=workdir result=done"
  echo "step=authorized_keys result=done"
  echo "step=gitconfig result=done"
  echo "step=copilot result=skipped"
  echo "step=dropin result=done"
  echo "step=sshd_reload result=done"
  rc="${PAIR_FAKE_APPLY_RC:-0}"
  if [ "$rc" = "3" ]; then
    echo "sshd -t failed: bad Match block" >&2
  fi
  exit "$rc"
fi
if grep -q '60-pulse-hands.conf.prev' "$ADMIN_STDIN"; then
  echo "step=dropin-prev result=removed"
  exit 0
fi
if grep -q 'restored-or-removed' "$ADMIN_STDIN"; then
  echo "step=dropin result=restored-or-removed"
  exit 0
fi
if [ "${PAIR_FAKE_SUDO_PASSWORD:-}" = "1" ]; then
  echo "sudo: a password is required" >&2
  exit 1
fi
cat <<EOF
uname=${PAIR_FAKE_UNAME:-Linux}
sudo=${PAIR_FAKE_SUDO:-yes}
user=${PAIR_FAKE_USER:-absent}
hands=${PAIR_FAKE_HANDS:-absent}
workdir=${PAIR_FAKE_WORKDIR:-absent}
workdir_owner=${PAIR_FAKE_WORKDIR_OWNER:-}
authorized_keys=${PAIR_FAKE_AUTHKEYS:-absent}
authorized_keys_lines=${PAIR_FAKE_AUTHKEY_LINES:-0}
dropin=${PAIR_FAKE_DROPIN:-absent}
dropin_dir=${PAIR_FAKE_DROPIN_DIR:-yes}
remotes=${PAIR_FAKE_REMOTES:-0}
apt=${PAIR_FAKE_APT:-yes}
copilot=${PAIR_FAKE_COPILOT:-present}
node=${PAIR_FAKE_NODE:-present}
git=${PAIR_FAKE_GIT:-present}
gitconfig=${PAIR_FAKE_GITCONFIG:-missing}
hostkeys=
hostkey ssh-ed25519 AAAATESTPAIR host-key-one
hostkey ssh-rsa AAAATESTPAIR2 host-key-two
EOF
"""


class PairFixture(tp.HandsFixture):
    def setUp(self):
        super().setUp()
        self.real_home = os.environ.get("HOME")
        self.fake_home = self.tmp / "home-user"
        self.fake_home.mkdir()
        os.environ["HOME"] = str(self.fake_home)
        self.admin_stdin = self.tmp / "admin-stdin"
        self.admin_stdin.mkdir()
        self.admin_log = self.tmp / "admin.log"
        handler = self.tmp / "admin-handler.sh"
        handler.write_text(PAIR_ADMIN_HANDLER, encoding="utf-8")
        handler.chmod(0o755)
        shim = Path(os.environ["PATH"].split(os.pathsep)[0]) / "ssh"
        shim.write_text(PAIR_SSH_SHIM, encoding="utf-8")
        shim.chmod(0o755)
        self._pair_env_before = {
            k: os.environ.get(k)
            for k in (
                "HOME", "PULSE_ADMIN_LOG", "PULSE_ADMIN_STDIN_DIR", "PULSE_ADMIN_HANDLER",
                "PAIR_FAKE_UNAME", "PAIR_FAKE_SUDO", "PAIR_FAKE_SUDO_PASSWORD",
                "PAIR_FAKE_AUTHKEYS", "PAIR_FAKE_AUTHKEY_LINES", "PAIR_FAKE_DROPIN_DIR",
                "PAIR_FAKE_REMOTES", "PAIR_FAKE_APT", "PAIR_FAKE_APPLY_RC",
                "PAIR_FAKE_RECHECK_RC", "PAIR_FAKE_COPILOT",
            )
        }
        os.environ["HOME"] = str(self.fake_home)
        os.environ["PULSE_ADMIN_LOG"] = str(self.admin_log)
        os.environ["PULSE_ADMIN_STDIN_DIR"] = str(self.admin_stdin)
        os.environ["PULSE_ADMIN_HANDLER"] = str(handler)
        for key in self._pair_env_before:
            if key.startswith("PAIR_FAKE_"):
                os.environ.pop(key, None)

    def tearDown(self):
        for key, value in self._pair_env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        if self.real_home is None:
            os.environ.pop("HOME", None)
        else:
            os.environ["HOME"] = self.real_home
        super().tearDown()

    def run_pair(self, *extra):
        return tp.run_cli("pair", "admin@box.example", "--name", "hands-2", "--now", NOW, *extra)

    def run_pair_for_probe(self, *extra):
        return self.run_pair("--workdir", str(self.remote), *extra)

    def admin_scripts(self):
        return sorted(self.admin_stdin.glob("*.sh"), key=lambda p: int(p.stem))

    def script_containing(self, needle):
        for path in self.admin_scripts():
            text = path.read_text(encoding="utf-8")
            if needle in text:
                return text
        self.fail("no saved admin stdin script contains %r" % needle)

    def assert_no_local_pair_state(self):
        self.assertFalse((self.fake_home / ".ssh" / "pulse_hands_ed25519").exists())
        self.assertFalse((self.fake_home / ".ssh" / "config").exists())
        self.assertIsNone(tp.pulse.executor_row(self.con, "hands-2"))
        self.assertEqual(self.con.execute("SELECT COUNT(*) FROM events WHERE action = 'executor.pair'").fetchone()[0], 0)


class PairScriptTest(tp.StoreFixture):
    def test_apply_script_is_byte_stable_and_shell_valid(self):
        # A fixed stand-in for pulse-hands.sh keeps the golden about the pairing steps, not about the hands' own text.
        fake_hands = "#!/bin/bash\necho fake pulse-hands\n"
        script = tp.pulse.pair_apply_script("hands-2", "/srv/pulse", "ssh-ed25519 AAAATEST pulse-hub", fake_hands, False)
        self.assertEqual(hashlib.sha256(script.encode("utf-8")).hexdigest(), APPLY_SCRIPT_SHA256,
                         "pair_apply_script changed; review ADR-021/cloud-init parity and update this hash intentionally")
        self.assertIn(fake_hands.strip(), script)
        real = tp.pulse.pair_apply_script("hands-2", "/srv/pulse", "ssh-ed25519 AAAATEST pulse-hub", tp.pulse.pair_hands_source(), False)
        self.assertIn(tp.HANDS_SH.read_text(encoding="utf-8").strip(), real)
        for text, label in ((script, "pair-apply.sh"), (real, "pair-apply-real.sh")):
            sample = self.tmp / label
            sample.write_text(text, encoding="utf-8")
            self.assertEqual(subprocess.run(["sh", "-n", str(sample)], capture_output=True, text=True).returncode, 0, label)


class PairCommandTest(PairFixture):
    def test_dry_run_prints_scripts_and_changes_nothing(self):
        code, out, err = self.run_pair("--dry-run")
        self.assertEqual(code, 0, err)
        for text in (
            "inspection:", "remote script (would run as root via sudo -n sh -s):",
            "undo script (the exact inverse):", "commit script (after successful admin re-check):",
            "Host hands-2", "known_hosts:", "executor add hands-2", "probe hands-2",
        ):
            self.assertIn(text, out)
        self.assert_no_local_pair_state()
        self.assertEqual(len(self.admin_scripts()), 1, "dry-run only performs read-only inspection")

    def test_refusals_write_nothing(self):
        cases = [
            ({"PAIR_FAKE_UNAME": "Darwin"}, (), "not Linux"),
            ({"PAIR_FAKE_AUTHKEYS": "other"}, (), "authorized_keys"),
            ({"PAIR_FAKE_DROPIN_DIR": "no"}, (), "manual sshd Match block"),
            ({"PAIR_FAKE_REMOTES": "1"}, (), "clone(s) with git remotes"),
            ({"PAIR_FAKE_APT": "no"}, ("--install-copilot",), "manual copilot install steps"),
            ({"PAIR_FAKE_SUDO_PASSWORD": "1"}, (), "sudo without a password is required"),
        ]
        for env, args, needle in cases:
            with self.subTest(env=env):
                for key in list(os.environ):
                    if key.startswith("PAIR_FAKE_"):
                        os.environ.pop(key, None)
                for key, value in env.items():
                    os.environ[key] = value
                shutil.rmtree(self.fake_home / ".ssh", ignore_errors=True)
                self.con.execute("DELETE FROM events WHERE action = 'executor.pair'")
                code, out, err = self.run_pair(*args)
                self.assertEqual(code, 2, out + err)
                self.assertIn(needle, out + err)
                self.assert_no_local_pair_state()

    def test_alias_pointing_elsewhere_refuses_before_remote_changes(self):
        ssh_dir = self.fake_home / ".ssh"
        ssh_dir.mkdir()
        (ssh_dir / "config").write_text("Host hands-2\n    HostName elsewhere.example\n", encoding="utf-8")
        code, out, err = self.run_pair()
        self.assertEqual(code, 2)
        self.assertIn("alias hands-2 already points elsewhere", out + err)
        self.assertFalse(self.admin_log.exists())

    def test_existing_executor_with_another_address_refuses(self):
        tp.pulse.add_executor(self.con, "hands-2", "pulse@old.example", "/srv/pulse", now=NOW)
        code, out, err = self.run_pair()
        self.assertEqual(code, 2)
        self.assertIn("exists with another address", out + err)
        self.assertFalse((self.fake_home / ".ssh" / "pulse_hands_ed25519").exists())

    def test_agent_without_decision_refuses_before_writes(self):
        tp.pulse.join_agent(self.con, "aurora", "r", "m")
        code, out, err = tp.run_cli("pair", "admin@box.example", "--name", "hands-2", "--agent", "aurora", "--now", NOW)
        self.assertEqual(code, 2)
        self.assertIn("no decided proposal names it", out + err)
        self.assert_no_local_pair_state()

    def test_agent_with_decision_succeeds_and_event_carries_post_id_only(self):
        tp.pulse.join_agent(self.con, "aurora", "r", "m")
        prop = tp.pulse.create_post(self.con, "answer", "proj", "runner", "pair", "we agree: pair hands-2",
                                    "The agents agreed.\n\npair: hands-2\n", agent="aurora", resolution="agreed")
        tp.pulse.decide(self.con, prop["id"], "yes", "go ahead")
        decided_at = self.con.execute("SELECT decided_at FROM decisions WHERE post_id = ?", (prop["id"],)).fetchone()["decided_at"]
        code, out, err = tp.run_cli("pair", "admin@box.example", "--name", "hands-2", "--workdir", str(self.remote),
                                    "--agent", "aurora", "--now", decided_at)
        self.assertEqual(code, 0, out + err)
        detail = json.loads(self.con.execute("SELECT detail FROM events WHERE action = 'executor.pair'").fetchone()["detail"])
        self.assertEqual(detail["decision"], prop["id"])
        self.assertNotIn("box.example", json.dumps(detail))
        self.assertNotIn("AAAATESTPAIR", json.dumps(detail))

    def test_sshd_t_failure_stops_before_local_state(self):
        os.environ["PAIR_FAKE_APPLY_RC"] = "3"
        code, out, err = self.run_pair_for_probe()
        self.assertEqual(code, 1, out + err)
        self.assertIn("sshd -t failed", out + err)
        apply = self.script_containing("/usr/local/bin/pulse-hands.tmp")
        self.assertIn('cp -p "$DROPIN" "$DROPIN.prev"', apply)
        self.assertIn('mv -f "$DROPIN.prev" "$DROPIN"', apply)
        self.assertFalse((self.fake_home / ".ssh" / "config").exists())
        self.assertIsNone(tp.pulse.executor_row(self.con, "hands-2"))

    def test_admin_recheck_failure_sends_undo_through_held_session(self):
        os.environ["PAIR_FAKE_RECHECK_RC"] = "255"
        code, out, err = self.run_pair_for_probe()
        self.assertEqual(code, 1, out + err)
        self.assertIn("admin path failed after reload", out)
        undo = self.script_containing("restored-or-removed")
        self.assertIn('mv -f "$DROPIN.prev" "$DROPIN"', undo)
        self.assertIn('rm -f "$DROPIN"', undo)
        self.assertFalse((self.fake_home / ".ssh" / "config").exists())
        self.assertIsNone(tp.pulse.executor_row(self.con, "hands-2"))

    def test_happy_path_is_idempotent_and_uses_alias_only(self):
        code, out, err = self.run_pair_for_probe()
        self.assertEqual(code, 0, out + err)
        key = self.fake_home / ".ssh" / "pulse_hands_ed25519"
        self.assertTrue(key.exists())
        self.assertEqual(stat.S_IMODE(key.stat().st_mode), 0o600)
        config = (self.fake_home / ".ssh" / "config").read_text(encoding="utf-8")
        self.assertEqual(config.count("Host hands-2"), 1)
        self.assertIn("HostName box.example", config)
        known = (self.fake_home / ".ssh" / "known_hosts").read_text(encoding="utf-8")
        self.assertIn("box.example ssh-ed25519 AAAATESTPAIR host-key-one", known)
        self.assertIn("box.example ssh-rsa AAAATESTPAIR2 host-key-two", known)
        row = tp.pulse.executor_row(self.con, "hands-2")
        self.assertEqual(row["address"], "pulse@hands-2")
        self.assertEqual(row["status"], "active")
        detail = self.con.execute("SELECT detail FROM events WHERE action = 'executor.pair'").fetchone()["detail"]
        self.assertNotIn("box.example", detail)
        self.assertNotIn("AAAATESTPAIR", detail)
        log = self.admin_log.read_text(encoding="utf-8")
        self.assertIn("ControlMaster=yes", log)
        self.assertIn("pulse-pair-commit", log)
        self.assertIn("-O exit", log)
        self.assertNotIn("pulse@box.example", log)
        commit = self.script_containing("dropin-prev")
        self.assertIn("60-pulse-hands.conf.prev", commit)

        code, out, err = self.run_pair_for_probe()
        self.assertEqual(code, 0, out + err)
        config2 = (self.fake_home / ".ssh" / "config").read_text(encoding="utf-8")
        self.assertEqual(config2.count("Host hands-2"), 1)
        self.assertEqual(tp.pulse.executor_row(self.con, "hands-2")["address"], "pulse@hands-2")
