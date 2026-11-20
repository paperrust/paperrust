"""CSRF: session-authenticated unsafe requests need a valid X-CSRFToken (DRF SessionAuthentication
enforce_csrf), token/basic auth does not. Probe: POST /api/documents/selection_data/ (read-only).
"""

from __future__ import annotations

from common import TargetCase, detail, login_session, need_user, session

PROBE = "/api/documents/selection_data/"
BODY = {"documents": []}


class Csrf(TargetCase):
    def test_session_post_without_csrf_header_rejected(self):
        s = login_session("admin")
        r = s.post(PROBE, json_body=BODY, csrf=False)
        self.assertEqual(r.status, 403)
        self.assertTrue((detail(r) or "").startswith("CSRF Failed"), detail(r))

    def test_session_post_with_wrong_csrf_rejected(self):
        s = login_session("admin")
        r = s.post(PROBE, json_body=BODY, csrf=False, headers={"X-CSRFToken": "x" * 64})
        self.assertEqual(r.status, 403)
        self.assertTrue((detail(r) or "").startswith("CSRF Failed"), detail(r))

    def test_session_post_without_csrf_cookie_rejected(self):
        s = login_session("admin")
        token = s.csrf_cookie()
        s.cookies = {k: v for k, v in s.cookies.items() if not k.endswith("csrftoken")}
        r = s.post(PROBE, json_body=BODY, csrf=False, headers={"X-CSRFToken": token or "x"})
        self.assertEqual(r.status, 403)

    def test_session_post_with_csrf_ok(self):
        s = login_session("admin")
        r = s.post(PROBE, json_body=BODY)
        self.assertEqual(r.status, 200, r.text[:200])

    def test_session_get_needs_no_csrf(self):
        s = login_session("admin")
        self.assertEqual(s.get("/api/ui_settings/", csrf=False).status, 200)

    def test_token_post_needs_no_csrf(self):
        need_user("admin")
        r = session("admin").post(PROBE, json_body=BODY, csrf=False)
        self.assertEqual(r.status, 200, r.text[:200])

    def test_basic_post_needs_no_csrf(self):
        u = need_user("admin")
        if not u.password:
            self.skipTest("no password")
        s = session()
        s.auth = ("basic", u.username, u.password)
        self.assertEqual(s.post(PROBE, json_body=BODY, csrf=False).status, 200)

    def test_login_form_post_without_csrf_rejected(self):
        u = need_user("admin")
        if not u.password:
            self.skipTest("no password")
        s = session()
        r = s.post("/accounts/login/", form={"login": u.username, "password": u.password}, csrf=False,
                   auth=False, api_version=False)
        self.assertEqual(r.status, 403)
        self.assertFalse(s.has_session())
