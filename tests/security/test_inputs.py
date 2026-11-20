"""Inputs and sandbox (docs/RELEASE-GATE.md section 4): path traversal via storage paths/titles,
template sandbox (Jinja: object access, endless loops, output bombs), zip slip on import,
open redirect after login, size and time limits, response headers of files.

Writing parts only against a throwaway DB (SEC_DB=pl_sec_*).
"""

from __future__ import annotations

import io
import os
import subprocess
import tempfile
import time
import unittest
import uuid
import zipfile
from pathlib import Path

from common import BASE_URL, REPO, TargetCase, need_user, session
from plharness.http import Session, session_login

WRITE_OK = os.environ.get("SEC_DB", "").startswith("pl_sec_")


def admin() -> Session:
    need_user("admin")
    return session("admin")


def a_document(s: Session) -> int:
    r = s.get("/api/documents/", query={"page_size": 1, "fields": "id"})
    return r.json()["results"][0]["id"]


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class StoragePathTraversal(TargetCase):
    BAD = ["../../../../etc/{{ title }}", "/etc/{{ title }}", "..\\..\\{{ title }}", "C:\\Windows\\{{ title }}",
           "{{ title }}/../../../x", "a/{{ '../' * 5 }}x"]

    def test_unsafe_paths_rejected_or_neutralised(self):
        s = admin()
        doc = a_document(s)
        for path in self.BAD:
            with self.subTest(path=path):
                r = s.post("/api/storage_paths/", json_body={"name": f"sec-{uuid.uuid4().hex[:6]}", "path": path,
                                                              "matching_algorithm": 0})
                self.assertLess(r.status, 500, r.text[:200])
                if r.status == 201:
                    sp = r.json()["id"]
                    t = s.post("/api/storage_paths/test/", json_body={"document": doc, "path": path})
                    self.assertLess(t.status, 500)
                    out = t.json() if t.status == 200 else None
                    if isinstance(out, str):
                        self.assertNotIn("..", out.replace("\\", "/").split("/"), f"traversal in rendered path {out!r}")
                        self.assertFalse(out.startswith(("/", "\\")) or (len(out) > 1 and out[1] == ":"),
                                         f"absolute rendered path {out!r}")
                    s.request("DELETE", f"/api/storage_paths/{sp}/")

    def test_title_with_traversal_rendered_safely(self):
        s = admin()
        doc = a_document(s)
        r = s.request("PATCH", f"/api/documents/{doc}/", json_body={"title": "../../../../sec-escape"})
        self.assertEqual(r.status, 200)
        t = s.post("/api/storage_paths/test/", json_body={"document": doc, "path": "x/{{ title }}"})
        self.assertEqual(t.status, 200, t.text[:200])
        out = t.json()
        if isinstance(out, str):
            self.assertNotIn("..", out.replace("\\", "/").split("/"), out)


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class TemplateSandbox(TargetCase):
    PAYLOADS = {
        "mro": "{{ ''.__class__.__mro__ }}",
        "globals": "{{ self.__init__.__globals__ }}",
        "config": "{{ config }}{{ settings.SECRET_KEY }}",
        "env": "{{ environ }}{{ os.environ }}",
        "loop": "{% for i in range(999999999) %}x{% endfor %}",
        "nested": "{% for a in range(100000) %}{% for b in range(100000) %}x{% endfor %}{% endfor %}",
        "bomb": "{{ 'x' * 1000000000 }}",
        "recursion": "{% macro m() %}{{ m() }}{% endmacro %}{{ m() }}",
        "owner_pw": "{{ document.owner.password }}{{ owner.password }}",
        # Fuzzing finding S-8: huge strftime width (previously ~1 GB memory or overflow)
        "strftime_width": "{{ created|datetime('%999999999d') }}{{ created|datetime('%99999999999999999999999d') }}",
        # Fuzzing: 16 MB output just below the limit (slow, but bounded)
        "big_output": "{{ 'a' * 104 * 5 * 1 * 104 * 5 * 10 * 6 }}",
    }

    def test_payloads_contained(self):
        s = admin()
        doc = a_document(s)
        for name, tpl in self.PAYLOADS.items():
            with self.subTest(payload=name):
                t0 = time.perf_counter()
                r = s.post("/api/storage_paths/test/", json_body={"document": doc, "path": tpl})
                dt = time.perf_counter() - t0
                self.assertLess(r.status, 500, r.text[:200])
                self.assertLess(dt, 10.0, f"template took {dt:.1f}s")
                body = r.text
                for marker in ("pbkdf2_sha256", "SECRET", "PAPERLESS_", "class '", "<module", "Traceback"):
                    self.assertNotIn(marker, body, f"{name} leaked {marker}")
                self.assertLess(len(r.body), 1_100_000, f"{name}: output {len(r.body)} bytes")

    def test_workflow_title_template(self):
        s = admin()
        r = s.post("/api/workflows/", json_body={
            "name": f"sec-{uuid.uuid4().hex[:6]}", "order": 0, "enabled": False,
            "triggers": [{"type": 1, "sources": [1], "filter_filename": "sec-none-*"}],
            "actions": [{"type": 1, "assign_title": "{{ ''.__class__.__mro__ }}{% for i in range(999999999) %}x{% endfor %}"}]})
        self.assertLess(r.status, 500, r.text[:200])
        if r.status == 201:
            s.request("DELETE", f"/api/workflows/{r.json()['id']}/")


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class ZipSlipImport(TargetCase):
    """document_importer unpacks the ZIP into the working folder before it checks the content.

    Runs against its own throwaway DB pl_sec_zipslip (the import empties content types/permissions of the target DB).
    """

    DB = "pl_sec_zipslip"

    @staticmethod
    def _psql(q: str) -> None:
        env_out = subprocess.run(["bash", "-c", f"source '{REPO}/reference/env.sh' >/dev/null 2>&1; "
                                  f"env PGPASSWORD=\"$PG_SUPERPASS\" \"$PG_BIN/psql\" -h 127.0.0.1 -p \"$PGPORT\" "
                                  f"-U \"$PG_SUPERUSER\" -d postgres -v ON_ERROR_STOP=1 -q -c \"{q}\""],
                                 capture_output=True, text=True)
        if env_out.returncode != 0:
            raise unittest.SkipTest(f"psql failed: {env_out.stderr[:200]}")

    def test_zip_slip(self):
        binp = REPO / "results" / "bin" / "paperrust-sec.exe"
        if not binp.exists():
            self.skipTest("results/bin/paperrust-sec.exe missing (tests/security/server.sh start)")
        self._psql(f"DROP DATABASE IF EXISTS {self.DB} WITH (FORCE)")
        self._psql(f"CREATE DATABASE {self.DB} TEMPLATE paperless_snapshot OWNER paperless")
        self.addCleanup(self._psql, f"DROP DATABASE IF EXISTS {self.DB} WITH (FORCE)")
        with tempfile.TemporaryDirectory() as td:
            td = Path(td)
            scratch = td / "scratch"
            scratch.mkdir()
            marker = f"sec-zipslip-{uuid.uuid4().hex[:6]}.txt"
            zp = td / "evil.zip"
            with zipfile.ZipFile(zp, "w") as z:
                z.writestr(f"../../{marker}", b"x")
                z.writestr(f"..\\..\\{marker}", b"x")
                z.writestr(f"/{marker}", b"x")
                z.writestr(f"C:/{marker}", b"x")
                z.writestr("manifest.json", b"[]")
            env = dict(os.environ, PAPERLESS_DBNAME=self.DB, PAPERLESS_SCRATCH_DIR=str(scratch),
                       PAPERLESS_SECRET_KEY=f"sec-test-{uuid.uuid4().hex}-local-only",
                       PAPERLESS_DBHOST="127.0.0.1", PAPERLESS_DBPORT="54329", PAPERLESS_DBUSER="paperless",
                       PAPERLESS_DBPASS="paperless", PAPERLESS_MEDIA_ROOT=str(td / "media"),
                       PAPERLESS_DATA_DIR=str(td / "data"))
            r = subprocess.run([str(binp), "manage", "document_importer", str(zp)], capture_output=True, text=True,
                               env=env, timeout=300, cwd=str(td))
            hits = [p for p in [td / marker, td.parent / marker, scratch.parent / marker, Path("C:/") / marker,
                                Path(td.anchor) / marker] if p.exists()]
            for p in hits:
                p.unlink()
            self.assertEqual(hits, [], f"zip-slip wrote outside the work folder: {hits}")
            self.assertNotIn("panicked", r.stderr)
            self.assertNotIn("configuration error", r.stdout + r.stderr, "importer did not run")


class OpenRedirect(TargetCase):
    def test_login_next_external(self):
        u = need_user("admin")
        for nxt in ("https://evil.invalid/", "//evil.invalid/", "/\\evil.invalid", "http:evil.invalid"):
            with self.subTest(next=nxt):
                s = Session(BASE_URL)
                path = "/accounts/login/?next=" + nxt
                ok = session_login(s, u.username, u.password, login_path=path)
                if not ok:
                    continue
                r = s.request("GET", path, auth=False, api_version=False)
                loc = r.header("location") or ""
                self.assertNotIn("evil.invalid", loc, f"open redirect via next={nxt}")


class Limits(TargetCase):
    def test_huge_json_body(self):
        s = admin()
        big = b'{"documents": [' + b"1," * 15_000_000 + b"1]}"
        try:
            r = s.request("POST", "/api/documents/selection_data/", data=big, content_type="application/json")
        except OSError:
            return  # connection closed by the server: acceptable limit
        self.assertIn(r.status, (400, 413), f"30 MB JSON accepted with {r.status}")

    def test_huge_query(self):
        s = admin()
        r = s.get("/api/documents/", query={"title__icontains": "a" * 200_000})
        self.assertLess(r.status, 500)

    def test_deep_json(self):
        s = admin()
        deep = "[" * 100_000 + "]" * 100_000
        r = s.request("POST", "/api/documents/selection_data/", data=deep.encode(), content_type="application/json")
        self.assertLess(r.status, 500)

    def test_slow_regex_matching_pattern(self):
        """User regex (mapping) with catastrophic backtracking must not block the server."""
        if not WRITE_OK:
            self.skipTest("needs a throwaway database")
        s = admin()
        r = s.post("/api/tags/", json_body={"name": f"sec-redos-{uuid.uuid4().hex[:6]}", "matching_algorithm": 4,
                                            "match": "(a+)+$|(x+x+)+y"})
        self.assertLess(r.status, 500)
        if r.status == 201:
            doc = a_document(s)
            t0 = time.perf_counter()
            r2 = s.get(f"/api/documents/{doc}/suggestions/")
            self.assertLess(time.perf_counter() - t0, 15.0)
            self.assertLess(r2.status, 500)
            s.request("DELETE", f"/api/tags/{r.json()['id']}/")


class FileResponses(TargetCase):
    def test_download_headers(self):
        s = admin()
        r = s.get("/api/documents/", query={"page_size": 200, "fields": "id,mime_type"})
        seen = set()
        for d in r.json()["results"]:
            if d["mime_type"] in seen:
                continue
            seen.add(d["mime_type"])
            for sub in ("download/", "preview/"):
                resp = s.get(f"/api/documents/{d['id']}/{sub}")
                if resp.status != 200:
                    continue
                with self.subTest(mime=d["mime_type"], sub=sub):
                    self.assertEqual((resp.header("x-content-type-options") or "").lower(), "nosniff")
                    ct = resp.content_type
                    self.assertNotIn(ct, ("text/html", "image/svg+xml", "application/xhtml+xml"),
                                     "active content served inline")
