"""Security headers like Django's middleware stack (settings: X_FRAME_OPTIONS = "SAMEORIGIN",
SecurityMiddleware defaults: nosniff, Referrer-Policy same-origin, COOP same-origin)."""

from __future__ import annotations

from common import TargetCase, need_user, session

EXPECTED = {
    "x-frame-options": "SAMEORIGIN",
    "x-content-type-options": "nosniff",
    "referrer-policy": "same-origin",
    "cross-origin-opener-policy": "same-origin",
}


class Headers(TargetCase):
    def test_api_response_headers(self):
        need_user("admin")
        for p in ("/api/ui_settings/", "/api/documents/?page_size=1", "/api/does-not-exist/"):
            r = session("admin").get(p)
            for k, v in EXPECTED.items():
                with self.subTest(path=p, header=k):
                    self.assertEqual(r.header(k), v)

    def test_anonymous_error_headers(self):
        r = session().get("/api/documents/")
        for k, v in EXPECTED.items():
            with self.subTest(header=k):
                self.assertEqual(r.header(k), v)

    def test_no_framework_banner(self):
        r = session().get("/api/")
        self.assertIsNone(r.header("x-powered-by"))
