"""Path traversal on file routes (logo, share, logs, static/assets, document files).

A probe passes when the response is not a 5xx, contains no file content of the probe
targets and no leak (paths, stack traces). Status 200 is allowed only for SPA/redirect
pages that do not contain the sensitive markers.
"""

from __future__ import annotations

from common import TargetCase, assert_no_leak, need_user, session

MARKERS = ["root:x:0:0", "[boot loader]", "[fonts]", "for 16-bit app support", "SECRET_KEY", "PAPERLESS_DBPASS",
           "DATABASES", "PAPERLESS_SECRET", "[extensions]", "-----BEGIN"]

PROBES = [
    "/logo/../../../../etc/passwd",
    "/logo/..%2F..%2F..%2F..%2Fetc%2Fpasswd",
    "/logo/%2e%2e/%2e%2e/%2e%2e/paperless.conf",
    "/logo/..%5C..%5C..%5Cwindows%5Cwin.ini",
    "/logo/C:%5CWindows%5Cwin.ini",
    "/logo//etc/passwd",
    "/logo/%252e%252e%252fetc%252fpasswd",
    "/logo/x%00.png",
    "/logo/....//....//etc/passwd",
    "/share/..%2F..%2Fetc%2Fpasswd",
    "/share/../../paperless.conf",
    "/api/logs/..%2F..%2Fpaperless.conf/",
    "/api/logs/%2e%2e/",
    "/api/logs/paperless.log/",
    "/api/logs/..%5Cpaperless/",
    "/static/../../../paperless.conf",
    "/static/..%2F..%2F..%2Fpaperless.conf",
    "/assets/..%2F..%2F..%2Fpaperless.conf",
    "/api/documents/%2e%2e/%2e%2e/etc/passwd",
    "/fetch/doc/1%2F..%2F..%2Fetc%2Fpasswd",
    "/api/documents/1/download/..%2F..%2F..%2Fetc%2Fpasswd",
]


class Traversal(TargetCase):
    def _probe(self, s, path, query=None):
        r = s.get(path, query=query)
        self.assertLess(r.status, 500, f"{path} -> {r.status}")
        body = r.text
        for m in MARKERS:
            self.assertNotIn(m, body, f"{path} returned sensitive content ({m})")
        assert_no_leak(self, r, what=path)

    def test_probes_anonymous(self):
        s = session()
        for p in PROBES:
            with self.subTest(path=p):
                self._probe(s, p)

    def test_probes_admin(self):
        need_user("admin")
        s = session("admin")
        for p in PROBES:
            with self.subTest(path=p):
                self._probe(s, p)

    def test_document_file_params(self):
        need_user("admin")
        s = session("admin")
        r = s.get("/api/documents/", query={"page_size": 1, "fields": "id"})
        if r.status != 200 or not r.json()["results"]:
            self.skipTest("no documents")
        doc = r.json()["results"][0]["id"]
        for sub in ("download/", "preview/", "thumb/"):
            for q in ({"original": "../../../../etc/passwd"}, {"version": "../../1"}, {"version": "-1"},
                      {"follow_formatting": "../../x"}):
                with self.subTest(sub=sub, query=q):
                    self._probe(s, f"/api/documents/{doc}/{sub}", q)

    def test_content_disposition_safe(self):
        need_user("admin")
        s = session("admin")
        r = s.get("/api/documents/", query={"page_size": 5, "fields": "id"})
        if r.status != 200:
            self.skipTest("no documents")
        for d in r.json()["results"]:
            resp = s.get(f"/api/documents/{d['id']}/download/")
            cd = resp.header("content-disposition") or ""
            with self.subTest(doc=d["id"]):
                self.assertNotIn("/", cd.split("filename", 1)[-1].replace("filename*=utf-8''", ""))
                self.assertNotIn("\\", cd)
                self.assertNotIn("\n", cd)
