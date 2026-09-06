#!/usr/bin/env python3
"""Owner-controlled guards for the PULSE dashboard (ADR-015, ADR-016).

This module is NOT part of the loop's editable `dashboard (product)` class
(`docs/evolution/constitution.md`): it lives under `docs/scripts/` without a
carve-out, so it is an immutable path and constraint C3 discards any variant
whose diff touches it. It judges `pulse-dashboard.py`; the editable test module
`test_pulse_dashboard.py` may add tests but never judges itself (evaluator P3,
maintainer decision 2026-09-05, board thread `governance/one-proposal`).

It imports nothing from the editable test module - its fixture is its own -
so a weakened helper there cannot weaken a guard here. Run alone with
`python3 -m unittest docs.scripts.tests.test_pulse_dashboard_guards`.
"""
import ast
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRIPTS = HERE.parent
MODULE = SCRIPTS / "pulse-dashboard.py"

# The exact policy the server must send. A variant that changes it changes a
# decision (ADR-015), not the product: the guard fails, the human decides.
CSP = ("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; "
       "img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
PAYLOAD = "<script>alert(1)</script><img src=x onerror=\"alert(2)\"> & \"quotes\" 'single'"
FORBIDDEN_JS = (b"eval(", b"new Function", b"document.write")  # on the served bundle; the framework runtime owns its innerHTML string
FORBIDDEN_SRC = ("dangerouslySetInnerHTML", "innerHTML", "outerHTML", "insertAdjacentHTML", "eval(", "new Function", "document.write",
                 "http://", "https://")  # on our source, where markup insertion and outside reach would be decided (ADR-023 §3)
W3C_NS = (b"http://www.w3.org/2000/svg", b"http://www.w3.org/1999/xhtml", b"http://www.w3.org/1998/Math/MathML", b"http://www.w3.org/XML/1998/namespace")
UI_SRC = MODULE.parent / "dashboard"


def load_module():
    spec = importlib.util.spec_from_file_location("pulse_dashboard_guarded", MODULE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha256_tree(root):
    """Hash every file under root (ignoring .git) so a write anywhere is visible."""
    digest = hashlib.sha256()
    for path in sorted(p for p in Path(root).rglob("*") if p.is_file() and ".git" not in p.parts):
        digest.update(str(path.relative_to(root)).encode())
        digest.update(path.read_bytes())
    return digest.hexdigest()


class GuardFixture:
    """A minimal PULSE repository: a board README, one ordinary post, one payload post."""

    def __init__(self, root):
        self.repo = Path(root) / "guard-repo"
        board = self.repo / "docs" / "board"
        (board / "governance" / "payload").mkdir(parents=True)
        (board / "README.md").write_text("# Board (guard fixture)\n", encoding="utf-8")
        self.payload_id = "20260901T090000Z-runner-x_model-x"
        (board / "governance" / "payload" / (self.payload_id + ".md")).write_text(
            "---\nkind: finding\narea: governance\nthread: payload\nagent: runner-x/model-x\n"
            "runner: runner-x\nmodel: model-x\nsession: s-x\ndate: 2026-09-01T09:00:00Z\nbranch: main\n"
            "links: []\nsummary: %s\n---\n\n## finding\n\n%s\n" % (PAYLOAD, PAYLOAD), encoding="utf-8")
        (board / "governance" / "plain").mkdir()
        (board / "governance" / "plain" / "20260902T090000Z-runner-y_model-y.md").write_text(
            "---\nkind: question\narea: governance\nthread: plain\nagent: runner-y/model-y\n"
            "runner: runner-y\nmodel: model-y\nsession: s-y\ndate: 2026-09-02T09:00:00Z\nbranch: main\n"
            "links: []\nsummary: A plain question\n---\n\n## question\n\nA plain question\n", encoding="utf-8")
        env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
                   GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")
        subprocess.run(["git", "init", "-q", str(self.repo)], env=env, check=True, capture_output=True)
        for cmd in (["git", "checkout", "-q", "-B", "main"], ["git", "add", "-A"],
                    ["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture"]):
            subprocess.run(cmd, cwd=str(self.repo), env=env, check=True, capture_output=True)
        self.tree_hash = sha256_tree(self.repo)

    def git_status(self):
        return subprocess.run(["git", "status", "--porcelain"], cwd=str(self.repo),
                              capture_output=True, text=True, check=True).stdout


def request(url, path, method="GET"):
    req = urllib.request.Request(url + path, method=method)
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, dict(resp.headers), resp.read()
    except urllib.error.HTTPError as exc:
        with exc:
            return exc.code, dict(exc.headers), exc.read()


class ServerGuards(unittest.TestCase):
    """ADR-015 invariants on a single-project server."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-guards-")).resolve()
        cls.env_before = {k: os.environ.pop(k, None) for k in ("PULSE_HOME", "PULSE_COMMONS", "PULSE_DASHBOARD_HOME")}
        os.environ["PULSE_HOME"] = str(cls.tmp / "home")
        cls.dash = load_module()
        cls.fx = GuardFixture(cls.tmp)
        cls.state_dir = cls.dash.project_state_dir(cls.fx.repo, cls.tmp / "state")
        cls.state = cls.dash.DashboardState(cls.fx.repo, cls.state_dir, refresh_seconds=3600)
        cls.state.ensure_fresh(force=True)
        cls.server = cls.dash.DashboardServer(cls.state, port=0)
        cls.thread = threading.Thread(target=cls.server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True)
        cls.thread.start()
        cls.url = cls.server.url.rstrip("/")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        for key, value in cls.env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_binds_loopback_only(self):
        self.assertEqual(self.server.server_address[0], "127.0.0.1")
        self.assertTrue(self.url.startswith("http://127.0.0.1:"))

    def test_security_headers_are_exact(self):
        for path in ("/", "/app.js", "/app.css", "/api/v1/overview", "/api/v1/nothing"):
            status, headers, _ = request(self.url, path)
            self.assertEqual(headers.get("Content-Security-Policy"), CSP, path)
            self.assertEqual(headers.get("X-Content-Type-Options"), "nosniff", path)
            self.assertEqual(headers.get("Cache-Control"), "no-store", path)
            self.assertEqual(headers.get("Referrer-Policy"), "no-referrer", path)
            self.assertEqual(headers.get("X-Frame-Options"), "DENY", path)

    def test_assets_never_inject_markup_or_reach_outside(self):
        status, headers, html = request(self.url, "/")
        self.assertEqual(status, 200)
        self.assertIn("text/html", headers["Content-Type"])
        self.assertNotIn(b"https://", html)
        self.assertNotIn(b"http://", html.replace(b"http://127.0.0.1", b""))
        self.assertNotIn(b"javascript:", html.lower())
        for tag in html.lower().split(b"<script")[1:]:
            self.assertIn(b"src=", tag.split(b">")[0], "every script tag loads a same-origin file; no inline script")
        self.assertNotRegex(html.decode("utf-8", "replace"), r"\son[a-z]+\s*=", "no inline event handlers")
        status, _, js = request(self.url, "/app.js")
        self.assertEqual(status, 200)
        for bad in FORBIDDEN_JS:
            self.assertNotIn(bad, js, "the bundle never evaluates text as code: %r" % bad)
        self.assertNotIn(b"https://", js)
        stripped = js
        for ns in W3C_NS:
            stripped = stripped.replace(ns, b"")
        self.assertNotIn(b"http://", stripped, "W3C namespace strings are the only http literals; never fetched")
        if UI_SRC.is_dir():  # ADR-023: the decision to insert markup or reach outside can only be made in our source
            for path in sorted(list((UI_SRC / "src").rglob("*")) + [UI_SRC / "index.html"]):
                if path.is_file() and path.suffix in (".ts", ".tsx", ".js", ".jsx", ".css", ".html"):
                    text = path.read_text(encoding="utf-8")
                    for bad in FORBIDDEN_SRC:
                        self.assertNotIn(bad, text, "%s: the page only ever inserts text and never reaches outside: %r" % (path.relative_to(UI_SRC), bad))
        self.assertNotIn(b"XMLHttpRequest", js.replace(b"fetch(", b""), "reads go through fetch to same-origin routes")
        status, headers, css = request(self.url, "/app.css")
        self.assertEqual(status, 200)
        self.assertIn("text/css", headers["Content-Type"])
        self.assertNotIn(b"@import", css)
        self.assertNotIn(b"url(http", css)
        self.assertNotIn(b"https://", css)

    def test_payload_stays_text(self):
        status, headers, body = request(self.url, "/api/v1/posts/" + self.fx.payload_id)
        self.assertEqual(status, 200)
        self.assertIn("application/json", headers["Content-Type"])
        data = json.loads(body)
        self.assertEqual(data["summary"], PAYLOAD, "the payload comes back as the same string inside JSON")
        self.assertIn(PAYLOAD, data["body"])
        self.assertNotIn("text/html", headers["Content-Type"])
        status, _, html = request(self.url, "/")
        self.assertNotIn(PAYLOAD.encode(), html, "the page is static; no board text is ever baked into it")
        status, _, threads = request(self.url, "/api/v1/threads")
        self.assertEqual(status, 200)
        json.loads(threads)

    def test_only_get_and_head_are_served(self):
        for path in ("/", "/api/v1/overview", "/api/v1/refresh", "/api/v1/posts/" + self.fx.payload_id):
            for method in ("POST", "PUT", "DELETE", "PATCH"):
                status, _, body = request(self.url, path, method=method)
                self.assertEqual(status, 405, "%s %s" % (method, path))
                self.assertIn("read-only", json.loads(body)["error"])
        status, _, body = request(self.url, "/", method="HEAD")
        self.assertEqual((status, body), (200, b""))

    def test_no_file_serving_or_path_traversal(self):
        for path in ("/docs/board/README.md", "/docs/board/governance/payload/%s.md" % self.fx.payload_id,
                     "/.git/config", "/..%2F..%2Fetc%2Fpasswd", "/api/v1/posts/..%2F..%2FREADME",
                     "/pulse-dashboard.py", "/board.sqlite"):
            status, _, _ = request(self.url, path)
            self.assertEqual(status, 404, path)

    def test_repository_is_never_written_and_state_lives_outside_it(self):
        for path in ("/", "/app.js", "/api/v1/overview", "/api/v1/threads", "/api/v1/refresh",
                     "/api/v1/posts/" + self.fx.payload_id):
            request(self.url, path)
        self.assertEqual(self.fx.git_status(), "", "serving never writes to the repository")
        self.assertEqual(sha256_tree(self.fx.repo), self.fx.tree_hash, "no tracked or untracked byte changed")
        self.assertFalse(self.dash.is_inside(self.state_dir, self.fx.repo), "the projection lives outside the repository")
        self.assertTrue((self.state_dir / self.dash.DB_NAME).is_file())


class HubGuards(unittest.TestCase):
    """ADR-016: the organization hub keeps the same invariants and never writes to a registry or a repository."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = Path(tempfile.mkdtemp(prefix="pulse-dashboard-hub-guards-")).resolve()
        cls.env_before = {k: os.environ.pop(k, None) for k in ("PULSE_HOME", "PULSE_COMMONS", "PULSE_DASHBOARD_HOME")}
        cls.home = cls.tmp / "home"
        cls.home.mkdir()
        os.environ["PULSE_HOME"] = str(cls.home)
        cls.dash = load_module()
        cls.fx = GuardFixture(cls.tmp)
        cls.dash.register_project(cls.home, cls.fx.repo)
        cls.registry_bytes = (cls.home / cls.dash.REGISTRY_NAME).read_bytes()
        cls.hub = cls.dash.HubState(cls.home, cls.tmp / "state", refresh_seconds=3600)
        cls.server = cls.dash.DashboardServer(None, port=0, hub=cls.hub)
        cls.thread = threading.Thread(target=cls.server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True)
        cls.thread.start()
        cls.url = cls.server.url.rstrip("/")

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()
        cls.server.server_close()
        for key, value in cls.env_before.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value
        shutil.rmtree(cls.tmp, ignore_errors=True)

    def test_hub_is_loopback_read_only_and_writes_nothing(self):
        self.assertEqual(self.server.server_address[0], "127.0.0.1")
        status, headers, body = request(self.url, "/api/v1/org")
        self.assertEqual(status, 200)
        self.assertEqual(headers.get("Content-Security-Policy"), CSP)
        org = json.loads(body)
        self.assertIn("guard-repo", [p["slug"] for p in org["projects"]])
        for path in ("/", "/p/guard-repo/", "/p/guard-repo/api/v1/overview", "/api/v1/notices", "/api/v1/proposals"):
            status, headers, _ = request(self.url, path)
            self.assertEqual(status, 200, path)
            self.assertEqual(headers.get("Content-Security-Policy"), CSP, path)
        for method in ("POST", "PUT", "DELETE", "PATCH"):
            for path in ("/api/v1/org", "/p/guard-repo/api/v1/overview"):
                status, _, body = request(self.url, path, method=method)
                self.assertEqual(status, 405, "%s %s" % (method, path))
                self.assertIn("read-only", json.loads(body)["error"])
        for path in ("/p/guard-repo/docs/board/README.md", "/p/guard-repo/.git/config", "/p/nope/api/v1/overview"):
            status, _, _ = request(self.url, path)
            self.assertEqual(status, 404, path)
        self.assertEqual(self.fx.git_status(), "", "the hub never writes to a repository")
        self.assertEqual(sha256_tree(self.fx.repo), self.fx.tree_hash)
        self.assertEqual((self.home / self.dash.REGISTRY_NAME).read_bytes(), self.registry_bytes,
                         "serving never rewrites the registry")


class SourceGuards(unittest.TestCase):
    """P2: the product stays standard-library only and keeps its exact policy string."""

    def test_module_imports_only_the_standard_library(self):
        tree = ast.parse(MODULE.read_text(encoding="utf-8"))
        names = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                names.update(alias.name.split(".")[0] for alias in node.names)
            elif isinstance(node, ast.ImportFrom) and node.module and node.level == 0:
                names.add(node.module.split(".")[0])
        outside = sorted(n for n in names if n not in sys.stdlib_module_names)
        self.assertEqual(outside, [], "pulse-dashboard.py must not depend on third-party packages")

    def test_policy_string_is_in_the_source_verbatim(self):
        source = MODULE.read_text(encoding="utf-8")
        self.assertIn("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; ", source)
        self.assertIn("img-src 'self' data:; base-uri 'none'; form-action 'none'; frame-ancestors 'none'", source)
        for verb in ("def do_PUT", "def do_DELETE", "def do_PATCH"):
            self.assertNotIn(verb, source, "write verbs may only alias the 405 handler; see the 405 test")


if __name__ == "__main__":
    unittest.main()
