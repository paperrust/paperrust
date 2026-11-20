"""Self-tests of the parity harness (no Django/Rust needed).

    python -m unittest tests/parity/harness/test_harness.py -v

Starts two tiny stub servers on 127.0.0.1 (ephemeral ports) that imitate a few DRF
responses, one of them with deliberate deviations, and checks diff, normalisation,
accepted rules, leak scan, loopback guard, user catalogue parsing and the report.
"""

from __future__ import annotations

import datetime as _dt
import json
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from plharness import cases as cases_mod  # noqa: E402
from plharness import report, rules, users  # noqa: E402
from plharness.config import Config  # noqa: E402
from plharness.diff import json_diff  # noqa: E402
from plharness.http import NonLoopbackTarget, Session, assert_loopback, encode_query  # noqa: E402
from plharness.leaks import LeakScanner  # noqa: E402
from plharness.normalize import Normalizer  # noqa: E402
from plharness.runner import Runner  # noqa: E402

TOKEN = "a" * 40


def make_handler(variant: str):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):  # silence
            pass

        def _send(self, status, obj, ctype="application/json", extra=None):
            body = obj if isinstance(obj, bytes) else json.dumps(obj).encode()
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Server", f"stub-{variant}")
            self.send_header("Vary", "Accept, Cookie" if variant == "a" else "Cookie, Accept")
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802
            auth = self.headers.get("Authorization", "")
            path = self.path.split("?")[0]
            if path == "/api/":
                return self._send(200, {"ok": True})
            if auth != f"Token {TOKEN}":
                return self._send(401, {"detail": "Authentication credentials were not provided."})
            now = _dt.datetime.now(_dt.timezone.utc).isoformat().replace("+00:00", "Z")
            if path == "/api/tags/":
                tags = [{"id": 1, "name": "a", "document_count": 3}]
                if variant == "b":
                    tags = [{"name": "a", "id": 1, "document_count": 4}]
                return self._send(200, {"count": 1, "next": None, "previous": None,
                                        "all": [1], "results": tags})
            if path == "/api/profile/":
                return self._send(200, {"username": "admin", "last_login": now,
                                        "created": "2020-01-01T00:00:00Z"})
            if path == "/api/leak/":
                if variant == "b":
                    return self._send(500, b"Traceback (most recent call last):\n  File \"x.py\", line 1",
                                      "text/plain")
                return self._send(404, {"detail": "Not found."})
            if path == "/api/first/":
                return self._send(200, {"results": [{"id": 7}, {"id": 9}]})
            return self._send(404, {"detail": "Not found."})

    return H


class StubServers(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.servers = []
        for v in ("a", "b"):
            srv = ThreadingHTTPServer(("127.0.0.1", 0), make_handler(v))
            threading.Thread(target=srv.serve_forever, daemon=True).start()
            cls.servers.append(srv)
        cls.a_url = f"http://127.0.0.1:{cls.servers[0].server_port}"
        cls.b_url = f"http://127.0.0.1:{cls.servers[1].server_port}"

    @classmethod
    def tearDownClass(cls):
        for s in cls.servers:
            s.shutdown()
            s.server_close()

    def _catalog(self, tmp: Path) -> Path:
        d = tmp / "pl-test"
        d.mkdir()
        (d / "cases.json").write_text(json.dumps({
            "crate": "pl-test",
            "vars": {"first_id": {"get": "/api/first/", "json": "results.0.id"},
                     "missing": {"get": "/api/nothing/", "json": "x"}},
            "cases": [
                {"id": "tags.list", "route": "tags.list", "path": "/api/tags/",
                 "query": {"page": 1, "ordering": "name"}, "users": ["admin", "anon"]},
                {"id": "profile", "route": "profile", "path": "/api/profile/"},
                {"id": "leak", "route": "leak", "path": "/api/leak/"},
                {"id": "uses-var", "route": "x", "path": "/api/x/${first_id}/"},
                {"id": "uses-missing", "route": "x", "path": "/api/x/${missing}/"},
                {"id": "unknown-user", "route": "x", "path": "/api/", "users": ["nobody"]},
                {"id": "w", "route": "w", "path": "/api/", "write": True},
            ],
        }), encoding="utf-8")
        (tmp / "accepted.json").write_text(json.dumps({"rules": [
            {"name": "test.doc-count", "reason": "test", "ref": "none", "case": "tags.list*",
             "kind": ["value"], "path": r"\$\.results\[\d+\]\.document_count"},
            {"name": "test.unused", "reason": "test", "ref": "none", "case": "zzz"},
        ]}), encoding="utf-8")
        return tmp

    def test_end_to_end(self):
        with tempfile.TemporaryDirectory() as t:
            tmp = Path(t)
            parity = self._catalog(tmp)
            cfg = Config(a_url=self.a_url, b_url=self.b_url, parity_dir=parity,
                         results_dir=tmp / "results", reset_cmd=None,
                         cutoff=_dt.datetime.now(_dt.timezone.utc) - _dt.timedelta(seconds=5))
            ucat = users.UserCatalog([users.User("admin", "admin", "pw-secret", TOKEN)], "test")
            rs = rules.RuleSet()
            rs.load_dir(parity)
            self.assertEqual(rs.errors, [])
            cat = cases_mod.load(parity)
            self.assertEqual(cat.errors, [])
            runner = Runner(cfg, ucat, rs, log=lambda *a: None)
            results = {r.id: r for r in runner.run(cat)}

            tl = results["tags.list@admin/v10"]
            self.assertEqual(tl.outcome, "different")  # key order differs (not accepted)
            kinds = {d["kind"] for d in tl.diffs}
            self.assertEqual(kinds, {"key-order"})
            self.assertEqual(tl.accepted[0]["rule"], "test.doc-count")
            self.assertEqual(results["tags.list@anon/v10"].outcome, "identical")
            # last_login is after the cut-off -> normalised; created before -> compared
            self.assertEqual(results["profile@admin/v10"].outcome, "identical")
            self.assertGreaterEqual(results["profile@admin/v10"].normalized.get("timestamp-after-cutoff", 0), 1)
            lk = results["leak@admin/v10"]
            self.assertEqual(lk.outcome, "different")
            self.assertTrue(any(x["category"] == "stacktrace" for x in lk.leaks_b))
            self.assertEqual(lk.leaks_a, [])
            self.assertEqual(results["uses-var@admin/v10"].steps[0]["url"].split("?")[0], "/api/x/7/")
            self.assertEqual(results["uses-missing@admin/v10"].outcome, "skipped")
            self.assertEqual(results["unknown-user@nobody/v10"].outcome, "skipped")
            self.assertIn("reset", results["w@admin/v10"].reason)

            jpath, mpath, summary = report.write(cfg, list(results.values()), rs, {"targets": "ab"},
                                                 _dt.datetime.now(_dt.timezone.utc))
            self.assertTrue(jpath.exists() and mpath.exists())
            self.assertEqual(summary["different"], 2)
            md = mpath.read_text(encoding="utf-8")
            self.assertIn("test.unused (unbenutzt)", md)
            self.assertIn("Historie", md)
            # the stub's token must never be copied into the report
            self.assertNotIn(TOKEN, jpath.read_text(encoding="utf-8"))

    def test_vary_order_insensitive(self):
        s = Session(self.a_url)
        r = s.get("/api/")
        n = Normalizer(_dt.datetime.now(_dt.timezone.utc))
        self.assertEqual(n.headers(r.headers)["vary"], "accept, cookie")


class Units(unittest.TestCase):
    def test_loopback_guard(self):
        assert_loopback("http://127.0.0.1:8000")
        assert_loopback("http://[::1]:8080/")
        assert_loopback("http://localhost:8080/")
        for bad in ("http://10.0.0.1/", "http://example.com/", "ftp://127.0.0.1/", "http://192.168.1.2:8000"):
            with self.assertRaises(NonLoopbackTarget):
                assert_loopback(bad)
        with self.assertRaises(NonLoopbackTarget):
            Session("http://8.8.8.8")

    def test_query_encoding(self):
        self.assertEqual(encode_query({"a": True, "b": [1, 2], "c": None, "d": "x,y"}), "a=true&b=1&b=2&d=x,y")

    def test_diff(self):
        d = json_diff({"a": 1, "b": [1, 2]}, {"b": [1], "a": 1.0, "c": None})
        kinds = sorted(x.kind for x in d)
        self.assertEqual(kinds, ["extra", "key-order", "length", "type"])
        self.assertEqual(json_diff(True, 1)[0].kind, "type")

    def test_normalize(self):
        cutoff = _dt.datetime(2026, 1, 1, tzinfo=_dt.timezone.utc)
        n = Normalizer(cutoff, write=True)
        out = n.json({"modified": "2026-02-01T10:00:00+01:00", "created": "2020-01-01",
                      "old": "2025-12-31T23:59:59Z", "task_id": "1b4e28ba-2fa1-11d2-883f-0016d3cca427",
                      "csrf_token": "abc"})
        self.assertEqual(out, {"modified": "<ts>", "created": "2020-01-01", "old": "2025-12-31T23:59:59Z",
                               "task_id": "<task-id>", "csrf_token": "<csrf>"})
        n2 = Normalizer(cutoff, write=False)
        self.assertEqual(n2.json({"task_id": "1b4e28ba-2fa1-11d2-883f-0016d3cca427"})["task_id"],
                         "1b4e28ba-2fa1-11d2-883f-0016d3cca427")
        self.assertIn("<csrf>", n.text('<input type="hidden" name="csrfmiddlewaretoken" value="XYZ">'))

    def test_leaks(self):
        sc = LeakScanner([TOKEN, "hunter22"], [r"C:\data\media"])
        found = {x.category for x in sc.scan_text(
            f'pbkdf2_sha256$870000$abc$def== {TOKEN} hunter22 "C:\\\\data\\\\media\\\\x.pdf" panicked at src/x.rs')}
        self.assertEqual(found, {"password-hash", "token", "password", "path", "stacktrace"})
        self.assertEqual(sc.scan_text("all good {\"password\": \"**********\"}"), [])
        self.assertEqual(sc.scan_text(TOKEN, allow={"token"}), [])

    def test_users_readme_table(self):
        md = ("| Nutzer | Rolle | Passwort | Token |\n|---|---|---|---|\n"
              f"| admin | superuser | adminpw | `{TOKEN}` |\n")
        cat = users._from_readme(md, "x")
        self.assertEqual(cat.get("admin").token, TOKEN)
        self.assertEqual(cat.get("admin").password, "adminpw")
        self.assertEqual(cat.resolve("admin@basic")[1], ("basic", "admin", "adminpw"))
        self.assertIsNone(cat.resolve("ghost"))
        block = '```json parity-users\n{"users": [{"name": "user1", "username": "alice", "token": "t"}]}\n```'
        self.assertEqual(users._from_readme(block, "x").get("user1").username, "alice")

    def test_rules_require_reason(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t) / "accepted.json"
            p.write_text(json.dumps({"rules": [{"name": "x"}]}), encoding="utf-8")
            rs = rules.RuleSet()
            rs.load_file(p)
            self.assertEqual(len(rs.rules), 0)
            self.assertEqual(len(rs.errors), 1)

    def test_sql_step_needs_private_db(self):
        import os
        from plharness import runner as runner_mod
        old = os.environ.pop("PARITY_SQL_DB", None)
        try:
            with self.assertRaises(runner_mod.SqlUnavailable):
                runner_mod.run_sql("select 1")
        finally:
            if old is not None:
                os.environ["PARITY_SQL_DB"] = old

    def test_substitute(self):
        self.assertEqual(cases_mod.substitute({"q": "${a}", "p": "/x/${a}/"}, {"a": 5}), {"q": 5, "p": "/x/5/"})
        with self.assertRaises(cases_mod.Unresolved):
            cases_mod.substitute("${zz}", {})


class SelectionPostOrder(unittest.TestCase):
    def test_post_selection_lists_sorted_by_id_only_top_level(self):
        keys = ("selected_correspondents", "selected_tags", "selected_document_types",
                "selected_storage_paths", "selected_custom_fields")
        post = {k: [{"id": 3, "document_count": 1}, {"id": 1, "document_count": 0}] for k in keys}
        n = Normalizer(_dt.datetime.now(_dt.timezone.utc))
        out = n.json(post)
        self.assertEqual([x["id"] for x in out["selected_tags"]], [1, 3])
        self.assertEqual(n.applied.get("selection-data-post-by-id"), 1)
        listing = {"count": 1, "results": [], "selection_data": post}
        out2 = Normalizer(_dt.datetime.now(_dt.timezone.utc)).json(listing)
        self.assertEqual([x["id"] for x in out2["selection_data"]["selected_tags"]], [3, 1])


if __name__ == "__main__":
    unittest.main()
