"""Authentication: token, basic, session, wrong/malformed credentials, inactive users.

Expected texts are DRF's (rest_framework/authentication.py) as used by upstream
(paperless/settings: PaperlessBasicAuthentication, TokenAuthentication, SessionAuthentication).
"""

from __future__ import annotations

from common import (NOT_PROVIDED, TargetCase, USERS, assert_no_leak, detail, login_session, need_user,
                    session)
from plharness.users import BAD_TOKEN

PROBE = "/api/ui_settings/"  # every authenticated user may read it, anon gets 401/403


class Auth(TargetCase):
    def test_anonymous_rejected(self):
        r = session().get("/api/documents/")
        self.assertIn(r.status, (401, 403))
        self.assertEqual(detail(r), NOT_PROVIDED)
        assert_no_leak(self, r)

    def test_anonymous_rejected_on_every_api_list(self):
        for p in ("/api/documents/", "/api/tags/", "/api/correspondents/", "/api/document_types/",
                  "/api/storage_paths/", "/api/custom_fields/", "/api/saved_views/", "/api/users/",
                  "/api/groups/", "/api/mail_accounts/", "/api/mail_rules/", "/api/workflows/",
                  "/api/tasks/", "/api/statistics/", "/api/logs/", "/api/config/", "/api/trash/",
                  "/api/share_links/", "/api/profile/", "/api/search/?query=a", "/api/status/"):
            with self.subTest(path=p):
                r = session().get(p)
                self.assertIn(r.status, (401, 403), p)

    def test_valid_token(self):
        need_user("admin")
        r = session("admin").get(PROBE)
        self.assertEqual(r.status, 200)

    def test_unknown_token(self):
        r = session().get(PROBE, headers={"Authorization": f"Token {BAD_TOKEN}"})
        self.assertEqual(r.status, 401)
        self.assertEqual(detail(r), "Invalid token.")

    def test_malformed_token_headers(self):
        cases = {
            "Token": "Invalid token header. No credentials provided.",
            "Token a b": "Invalid token header. Token string should not contain spaces.",
        }
        for header, text in cases.items():
            with self.subTest(header=header):
                r = session().get(PROBE, headers={"Authorization": header})
                self.assertEqual(r.status, 401)
                self.assertEqual(detail(r), text)

    def test_other_schemes_ignored(self):
        for header in ("Bearer abc", f"token{BAD_TOKEN}", "Digest x"):
            with self.subTest(header=header):
                r = session().get(PROBE, headers={"Authorization": header})
                self.assertIn(r.status, (401, 403))

    def test_token_not_accepted_in_query(self):
        u = need_user("admin")
        r = session().get(PROBE, query={"token": u.token, "auth_token": u.token})
        self.assertIn(r.status, (401, 403))

    def test_basic_auth(self):
        u = need_user("admin")
        if not u.password:
            self.skipTest("no password for admin")
        s = session()
        s.auth = ("basic", u.username, u.password)
        self.assertEqual(s.get(PROBE).status, 200)
        s.auth = ("basic", u.username, u.password + "x")
        r = s.get(PROBE)
        self.assertEqual(r.status, 401)
        self.assertEqual(detail(r), "Invalid username/password.")
        s.auth = ("raw", "Basic !!!notbase64")
        r = s.get(PROBE)
        self.assertEqual(r.status, 401)
        self.assertEqual(detail(r), "Invalid basic header. Credentials not correctly base64 encoded.")

    def test_inactive_user_token(self):
        u = need_user("inactive")
        if not u.token:
            self.skipTest("inactive user has no token")
        r = session("inactive").get(PROBE)
        self.assertEqual(r.status, 401)
        self.assertEqual(detail(r), "User inactive or deleted.")

    def test_inactive_user_basic(self):
        u = need_user("inactive")
        if not u.password:
            self.skipTest("inactive user has no password")
        s = session()
        s.auth = ("basic", u.username, u.password)
        r = s.get(PROBE)
        self.assertEqual(r.status, 401)

    def test_session_login(self):
        s = login_session("admin")
        r = s.get(PROBE)
        self.assertEqual(r.status, 200)
        assert_no_leak(self, r)

    def test_session_login_wrong_password(self):
        u = need_user("admin")
        if not u.password:
            self.skipTest("no password")
        from plharness.http import session_login
        s = session()
        self.assertFalse(session_login(s, u.username, u.password + "x"))
        self.assertIn(s.get(PROBE).status, (401, 403))

    def test_forged_session_cookie(self):
        s = session()
        s.cookies["sessionid"] = "x" * 32
        r = s.get(PROBE)
        self.assertIn(r.status, (401, 403))

    def test_session_survives_only_with_cookie(self):
        s = login_session("admin")
        s.cookies = {k: v for k, v in s.cookies.items() if not k.endswith("sessionid")}
        self.assertIn(s.get(PROBE).status, (401, 403))

    def test_catalogue_present(self):
        self.assertTrue(USERS.users, f"no users loaded ({USERS.source})")
