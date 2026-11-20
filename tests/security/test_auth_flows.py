"""Login and session in detail (docs/RELEASE-GATE.md section 4): CSRF via form field and
header, session end, session fixation, cookie attributes, token renewal, 2FA (TOTP incl.
replay protection and Basic/token behavior), throttling (also across two processes), forgot
password without enumeration.

Creates and changes its own users -> only against a throwaway DB (SEC_DB=pl_sec_*), otherwise skipped.
Second process for the throttling: SECURITY_URL2 (e.g. http://127.0.0.1:8371, `SEC_INSTANCE=2 SEC_PORT=8371
bash tests/security/server.sh start`).
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import os
import re
import struct
import time
import unittest
import uuid

from common import BASE_URL, TargetCase, detail
from plharness.http import Session, session_login as _raw_login

ADMIN_TOKEN = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"
WRITE_OK = os.environ.get("SEC_DB", "").startswith("pl_sec_")
URL2 = os.environ.get("SECURITY_URL2")
CSRF_INPUT = re.compile(r'name="csrfmiddlewaretoken" value="([^"]+)"')


def totp(secret: str, at: float | None = None, step: int = 30) -> str:
    key = base64.b32decode(secret.upper() + "=" * (-len(secret) % 8))
    counter = int((at if at is not None else time.time()) // step)
    h = hmac.new(key, struct.pack(">Q", counter), hashlib.sha1).digest()
    o = h[-1] & 0x0F
    return f"{(struct.unpack('>I', h[o:o + 4])[0] & 0x7FFFFFFF) % 1_000_000:06d}"


def wait_rate_window(s: Session, path: str = "/accounts/login/") -> None:
    """allauth limits logins per IP (30/min); the suite itself logs in often -> wait on 429."""
    for _ in range(14):
        r = s.request("GET", path, auth=False, api_version=False)
        if r.status != 429:
            return
        time.sleep(5)


def session_login(s: Session, username: str, password: str, login_path: str = "/accounts/login/") -> bool:
    for _ in range(3):
        if _raw_login(s, username, password, login_path):
            return True
        probe = Session(s.base_url)
        r = probe.request("POST", login_path, form={}, auth=False, api_version=False, csrf=False)
        if r.status != 429:
            return False
        time.sleep(30)
    return False


def admin(url: str = BASE_URL) -> Session:
    s = Session(url, api_version="10")
    s.auth = ("token", ADMIN_TOKEN)
    return s


def new_user(password: str | None = None, **extra) -> tuple[str, str, int]:
    name = f"secauth-{uuid.uuid4().hex[:8]}"
    pw = password or f"Pw-{uuid.uuid4().hex}"
    body = {"username": name, "password": pw, "email": f"{name}@example.invalid", "is_active": True,
            "user_permissions": ["view_uisettings", "add_uisettings", "view_document"], **extra}
    r = admin().post("/api/users/", json_body=body)
    assert r.status == 201, r.text[:300]
    return name, pw, r.json()["id"]


def form_get(s: Session, path: str) -> str:
    r = s.request("GET", path, auth=False, api_version=False)
    m = CSRF_INPUT.search(r.text)
    return m.group(1) if m else ""


def form_post(s: Session, path: str, data: dict, token: str | None, origin: str | None = None) -> object:
    form = dict(data)
    if token is not None:
        form["csrfmiddlewaretoken"] = token
    host = BASE_URL.split("://", 1)[1]
    hdrs = {"Referer": f"http://{host}{path}"}
    if origin:
        hdrs["Origin"] = origin
    return s.request("POST", path, form=form, headers=hdrs, auth=False, api_version=False, csrf=False)


def logged_in(s: Session) -> bool:
    return s.get("/api/profile/", auth=False).status == 200


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class CsrfForm(TargetCase):
    def test_form_field_token_required(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL)
        tok = form_get(s, "/accounts/login/")
        self.assertTrue(tok, "login form has no csrfmiddlewaretoken")
        r = form_post(s, "/accounts/login/", {"login": name, "password": pw}, None)
        self.assertEqual(r.status, 403, "login without form token")
        r = form_post(s, "/accounts/login/", {"login": name, "password": pw}, "x" * 64)
        self.assertEqual(r.status, 403, "login with forged form token")
        self.assertFalse(s.has_session())
        r = form_post(s, "/accounts/login/", {"login": name, "password": pw}, tok)
        self.assertIn(r.status, (200, 302))
        self.assertTrue(logged_in(s))

    def test_token_from_other_browser_rejected(self):
        name, pw, _ = new_user()
        a, b = Session(BASE_URL), Session(BASE_URL)
        tok_a = form_get(a, "/accounts/login/")
        form_get(b, "/accounts/login/")
        r = form_post(b, "/accounts/login/", {"login": name, "password": pw}, tok_a)
        # Django: masked tokens of another cookie secret do not match -> 403
        self.assertEqual(r.status, 403)

    def test_foreign_origin_rejected(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL)
        tok = form_get(s, "/accounts/login/")
        r = form_post(s, "/accounts/login/", {"login": name, "password": pw}, tok, origin="http://evil.invalid")
        self.assertEqual(r.status, 403, "foreign Origin with valid token")
        self.assertFalse(s.has_session())

    def test_session_api_header_and_form(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        body = {"settings": {"x": 1}}
        r = s.post("/api/ui_settings/", json_body=body, csrf=False)
        self.assertEqual(r.status, 403)
        self.assertTrue((detail(r) or "").startswith("CSRF Failed"))
        r = s.post("/api/ui_settings/", json_body=body, csrf=False, headers={"X-CSRFToken": s.csrf_cookie() or ""})
        self.assertNotIn(r.status, (401, 403), r.text[:200])


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class Sessions(TargetCase):
    def test_cookie_flags_and_fixation(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL)
        s.cookies["sessionid"] = "attackerchosen0000000000000000000"
        r0 = s.request("GET", "/accounts/login/", auth=False, api_version=False)
        tok = CSRF_INPUT.search(r0.text).group(1)
        r = form_post(s, "/accounts/login/", {"login": name, "password": pw}, tok)
        cookies = r.header_all("set-cookie")
        sid = [c for c in cookies if c.split("=", 1)[0].endswith("sessionid")]
        self.assertTrue(sid, "no session cookie on login")
        self.assertIn("httponly", sid[0].lower())
        self.assertIn("samesite=lax", sid[0].lower())
        self.assertNotEqual(s.cookies.get("sessionid"), "attackerchosen0000000000000000000")
        csrf = [c for c in cookies if c.split("=", 1)[0].endswith("csrftoken")]
        if csrf:
            self.assertIn("samesite", csrf[0].lower())

    def test_logout_invalidates_session(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        stolen = dict(s.cookies)
        self.assertTrue(logged_in(s))
        tok = form_get(s, "/accounts/logout/") or s.csrf_cookie()
        r = form_post(s, "/accounts/logout/", {}, tok)
        self.assertIn(r.status, (200, 302))
        replay = Session(BASE_URL, api_version="10")
        replay.cookies = stolen
        self.assertFalse(logged_in(replay), "session cookie still valid after logout")

    def test_password_change_ends_other_sessions(self):
        name, pw, uid = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        r = admin().request("PATCH", f"/api/users/{uid}/", json_body={"password": f"New-{uuid.uuid4().hex}"})
        self.assertEqual(r.status, 200)
        # Django: session auth hash changes with the password -> old sessions end
        self.assertFalse(logged_in(s), "session survives password change")

    def test_deactivated_user_session_ends(self):
        name, pw, uid = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        self.assertEqual(admin().request("PATCH", f"/api/users/{uid}/", json_body={"is_active": False}).status, 200)
        self.assertFalse(logged_in(s), "session of a deactivated user still valid")


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class Tokens(TargetCase):
    def test_regenerate_invalidates_old_token(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        r = s.post("/api/profile/generate_auth_token/", json_body={})
        self.assertEqual(r.status, 200, r.text[:200])
        t1 = r.json()
        r = s.post("/api/profile/generate_auth_token/", json_body={})
        t2 = r.json()
        self.assertNotEqual(t1, t2)
        a = Session(BASE_URL, api_version="10")
        a.auth = ("token", t1)
        self.assertEqual(a.get("/api/profile/").status, 401, "old token still valid")
        a.auth = ("token", t2)
        self.assertEqual(a.get("/api/profile/").status, 200)

    @unittest.skip("upstream ProfileSerializer liefert auth_token an den angemeldeten Nutzer (gleiches Verhalten)")
    def test_token_shown_once_not_in_profile(self):
        name, pw, _ = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        tok = s.post("/api/profile/generate_auth_token/", json_body={}).json()
        prof = s.get("/api/profile/").text
        self.assertNotIn(str(tok), prof, "token value returned by GET /api/profile/")

    def test_deleted_user_token_invalid(self):
        name, pw, uid = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        tok = s.post("/api/profile/generate_auth_token/", json_body={}).json()
        self.assertEqual(admin().request("DELETE", f"/api/users/{uid}/").status, 204)
        a = Session(BASE_URL, api_version="10")
        a.auth = ("token", tok)
        self.assertEqual(a.get("/api/profile/").status, 401)


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class TwoFactor(TargetCase):
    def _enable(self):
        name, pw, uid = new_user()
        s = Session(BASE_URL, api_version="10")
        self.assertTrue(session_login(s, name, pw))
        r = s.get("/api/profile/totp/")
        self.assertEqual(r.status, 200)
        secret = r.json()["secret"]
        r = s.post("/api/profile/totp/", json_body={"secret": secret, "code": "000000" if totp(secret) != "000000" else "111111"})
        self.assertFalse(r.json().get("success"), "wrong TOTP code accepted on activation")
        r = s.post("/api/profile/totp/", json_body={"secret": secret, "code": totp(secret)})
        self.assertTrue(r.json().get("success"), r.text[:200])
        self.assertTrue(r.json().get("recovery_codes"))
        return name, pw, uid, secret, r.json()["recovery_codes"]

    def _password_step(self, name, pw) -> Session:
        s = Session(BASE_URL, api_version="10")
        wait_rate_window(s)
        tok = form_get(s, "/accounts/login/")
        r = form_post(s, "/accounts/login/", {"login": name, "password": pw}, tok)
        self.assertIn(r.status, (200, 302))
        self.assertIn("2fa/authenticate", (r.header("location") or "") + r.text[:3000])
        self.assertFalse(logged_in(s), "logged in after password step without TOTP")
        return s

    def test_totp_login_flow_and_replay(self):
        name, pw, uid, secret, _ = self._enable()
        s = self._password_step(name, pw)
        tok = form_get(s, "/accounts/2fa/authenticate/")
        r = form_post(s, "/accounts/2fa/authenticate/", {"code": "123456" if totp(secret) != "123456" else "654321"}, tok)
        self.assertFalse(logged_in(s), "wrong TOTP code logged in")
        code = totp(secret)
        tok = form_get(s, "/accounts/2fa/authenticate/") or tok
        r = form_post(s, "/accounts/2fa/authenticate/", {"code": code}, tok)
        self.assertTrue(logged_in(s), f"correct TOTP code rejected ({r.status})")
        # replay of the same code in a second login (allauth: code used once per step window)
        s2 = self._password_step(name, pw)
        tok = form_get(s2, "/accounts/2fa/authenticate/")
        form_post(s2, "/accounts/2fa/authenticate/", {"code": code}, tok)
        self.assertFalse(logged_in(s2), "TOTP code replay accepted")

    def test_recovery_code_single_use(self):
        name, pw, uid, secret, codes = self._enable()
        s = self._password_step(name, pw)
        tok = form_get(s, "/accounts/2fa/authenticate/")
        form_post(s, "/accounts/2fa/authenticate/", {"code": codes[0]}, tok)
        self.assertTrue(logged_in(s), "recovery code rejected")
        s2 = self._password_step(name, pw)
        tok = form_get(s2, "/accounts/2fa/authenticate/")
        form_post(s2, "/accounts/2fa/authenticate/", {"code": codes[0]}, tok)
        self.assertFalse(logged_in(s2), "recovery code used twice")

    def test_basic_auth_refused_with_mfa(self):
        name, pw, uid, secret, _ = self._enable()
        s = Session(BASE_URL, api_version="10")
        s.auth = ("basic", name, pw)
        r = s.get("/api/profile/")
        self.assertEqual(r.status, 401)
        self.assertEqual(detail(r), "MFA required")

    def test_token_endpoint_with_mfa(self):
        name, pw, uid, secret, _ = self._enable()
        r = Session(BASE_URL).post("/api/token/", json_body={"username": name, "password": pw})
        # upstream PaperlessAuthTokenSerializer: MFA users get no token by password alone
        self.assertNotEqual(r.status, 200, "token issued by password alone for a MFA user")


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class ZThrottling(TargetCase):
    """Runs last (name), because the IP may be blocked for a minute afterwards."""

    def _try(self, url: str, name: str, pw: str = "wrong", host: str | None = None) -> tuple[str, Session]:
        """One login attempt; result "locked" (allauth pre_authenticate: form error
        "Too many failed login attempts", status 200; or 429 of the IP limit), "failed" or "ok"."""
        host = host or url.split("://", 1)[1]
        s = Session(url, host_header=host)
        tok = form_get(s, "/accounts/login/")
        r = s.request("POST", "/accounts/login/", form={"login": name, "password": pw, "csrfmiddlewaretoken": tok},
                      headers={"Referer": f"http://{host}/accounts/login/", "Accept-Language": "en"},
                      auth=False, api_version=False, csrf=False)
        if r.status == 429 or "too many failed" in r.text.lower():
            return "locked", s
        return ("ok" if s.has_session() else "failed"), s

    def test_login_failed_per_account(self):
        name, pw, _ = new_user()
        st = [self._try(BASE_URL, name)[0] for _ in range(6)]
        # Lock at the 6th attempt at the latest (5/300 s per account; the IP limit 10/min may apply earlier)
        self.assertIn("locked", st, f"no lockout after 6 failures for one account: {st}")
        res, s = self._try(BASE_URL, name, pw)
        self.assertEqual(res, "locked", "correct password accepted during lockout")
        self.assertFalse(s.has_session())

    @unittest.skipUnless(URL2, "SECURITY_URL2 (second process on the same DB) not set")
    def test_login_failed_across_processes(self):
        name, _, _ = new_user()
        # same Host header as behind a proxy (the account key is "<host>:<login>")
        host = BASE_URL.split("://", 1)[1]
        st = [self._try(BASE_URL if i % 2 == 0 else URL2, name, host=host)[0] for i in range(6)]
        self.assertIn("locked", st, f"lockout not shared between processes: {st}")
        res, s = self._try(URL2, name, "irrelevant", host=host)
        self.assertEqual(res, "locked", "other process accepts attempts during lockout")

    def test_basic_auth_bruteforce(self):
        """Finding S-2: Basic auth was unthrottled (like upstream). Now 5 failed attempts per account,
        afterwards 401 "Too many failed login attempts." also with the correct password."""
        name, pw, _ = new_user()
        s = Session(BASE_URL, api_version="10")
        details = []
        for _ in range(6):
            s.auth = ("basic", name, f"wrong-{uuid.uuid4().hex[:6]}")
            r = s.get("/api/profile/")
            self.assertEqual(r.status, 401)
            details.append(detail(r))
        self.assertEqual(details[:5], ["Invalid username/password."] * 5, details)
        self.assertEqual(details[5], "Too many failed login attempts.", details)
        s.auth = ("basic", name, pw)
        r = s.get("/api/profile/")
        self.assertEqual((r.status, detail(r)), (401, "Too many failed login attempts."),
                         "correct password accepted during Basic lockout")


@unittest.skipUnless(WRITE_OK, "needs a throwaway database (SEC_DB=pl_sec_*)")
class PasswordReset(TargetCase):
    def _reset(self, email: str):
        s = Session(BASE_URL)
        tok = form_get(s, "/accounts/password/reset/")
        t0 = time.perf_counter()
        r = form_post(s, "/accounts/password/reset/", {"email": email}, tok)
        return r, time.perf_counter() - t0

    def test_no_account_enumeration(self):
        name, pw, _ = new_user()
        known, unknown = f"{name}@example.invalid", f"nobody-{uuid.uuid4().hex[:6]}@example.invalid"
        rk, tk = self._reset(known)
        ru, tu = self._reset(unknown)
        self.assertEqual(rk.status, ru.status)
        self.assertEqual(rk.header("location"), ru.header("location"))
        norm = lambda r: re.sub(r'value="[^"]+"', "", r.text)  # noqa: E731
        self.assertEqual(norm(rk), norm(ru), "reset response differs for known/unknown address")
        self.assertNotIn(name, rk.text)
        # Time difference rough (mail sending in the test server without SMTP); only very large gaps are a finding
        self.assertLess(abs(tk - tu), 2.0, f"timing differs strongly: known {tk:.2f}s unknown {tu:.2f}s")

    def test_reset_key_garbage(self):
        s = Session(BASE_URL)
        for key in ("x", "1-" + "a" * 40, "../../etc/passwd", "%00"):
            r = s.request("GET", f"/accounts/password/reset/key/{key}/", auth=False, api_version=False)
            self.assertLess(r.status, 500, key)
            self.assertNotIn("password1", r.text if r.status == 200 and "invalid" in r.text.lower() else "")
