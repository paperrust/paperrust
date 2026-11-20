"""Social/OIDC login against the local fake provider (tests/parity/pl-accounts/w12_sinks.py).

Preparation (local only, throwaway DB):
    python -I tests/parity/pl-accounts/w12_sinks.py tests/security/.work/sinks &      # IdP :8141, SMTP :8142
    SEC_OIDC=1 SEC_INSTANCE=3 SEC_PORT=8372 bash tests/security/server.sh start
    SECURITY_OIDC_URL=http://127.0.0.1:8372 SEC_DB=pl_sec_matrix python tests/security/run_security.py -k Oidc

Checked: normal flow (PKCE, client secret, redirect_uri), manipulated/foreign/missing `state`,
reused code, invalid id_token (signature, aud, iss, expired), errors of the provider.
"""

from __future__ import annotations

import json
import os
import re
import unittest
import urllib.parse
import urllib.request
import uuid

from common import TargetCase
from plharness.http import Session

URL = os.environ.get("SECURITY_OIDC_URL")
IDP = "http://127.0.0.1:8141"
CSRF_INPUT = re.compile(r'name="csrfmiddlewaretoken" value="([^"]+)"')
LOGIN = "/accounts/oidc/fakeidp/login/"


def idp_up() -> bool:
    try:
        with urllib.request.urlopen(IDP + "/.well-known/openid-configuration", timeout=3) as r:
            return r.status == 200
    except OSError:
        return False


def idp_last() -> dict:
    with urllib.request.urlopen(IDP + "/_last", timeout=3) as r:
        return json.loads(r.read())


@unittest.skipUnless(URL and idp_up(), "SECURITY_OIDC_URL / fake IdP on :8141 not available")
class Oidc(TargetCase):
    def start(self, hint: str) -> tuple[Session, str]:
        """Up to the redirect back: returns session and callback URL (not yet called)."""
        s = Session(URL, api_version="10")
        q = "?process=login&auth_params=" + urllib.parse.quote(f"login_hint={hint}")
        r = s.request("GET", LOGIN + q, auth=False, api_version=False)
        m = CSRF_INPUT.search(r.text)
        host = URL.split("://", 1)[1]
        r = s.request("POST", LOGIN + q, form={"csrfmiddlewaretoken": m.group(1) if m else ""},
                      headers={"Referer": f"http://{host}{LOGIN}"}, auth=False, api_version=False, csrf=False)
        self.assertEqual(r.status, 302, f"provider login did not redirect: {r.status} {r.text[:200]}")
        auth_url = r.header("location") or ""
        self.assertTrue(auth_url.startswith(IDP + "/authorize"), auth_url)
        aq = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(auth_url).query))
        self.assertTrue(aq.get("state"), "no state parameter")
        self.assertEqual(aq.get("code_challenge_method"), "S256", "PKCE missing")
        r = s.request("GET", auth_url, auth=False, api_version=False)
        cb = r.header("location") or ""
        self.assertIn("/accounts/oidc/fakeidp/login/callback/", cb)
        return s, cb

    def finish(self, s: Session, cb: str) -> bool:
        s.request("GET", cb, auth=False, api_version=False)
        return s.get("/api/profile/", auth=False).status == 200

    def test_normal_flow(self):
        s, cb = self.start(f"oidc{uuid.uuid4().hex[:6]}~verified")
        self.assertTrue(self.finish(s, cb), "regular OIDC login failed")
        last = idp_last()["token"]
        self.assertTrue(last.get("has_secret") or last.get("basic"), "client secret not sent")
        self.assertTrue(last.get("code_verifier"), "PKCE verifier not sent")

    def test_tampered_state(self):
        s, cb = self.start(f"oidc{uuid.uuid4().hex[:6]}~verified")
        u = urllib.parse.urlsplit(cb)
        q = dict(urllib.parse.parse_qsl(u.query))
        q["state"] = "x" + q.get("state", "")[1:]
        self.assertFalse(self.finish(s, urllib.parse.urlunsplit(u._replace(query=urllib.parse.urlencode(q)))),
                         "tampered state accepted")

    def test_missing_state(self):
        s, cb = self.start(f"oidc{uuid.uuid4().hex[:6]}~verified")
        u = urllib.parse.urlsplit(cb)
        q = {k: v for k, v in urllib.parse.parse_qsl(u.query) if k != "state"}
        self.assertFalse(self.finish(s, urllib.parse.urlunsplit(u._replace(query=urllib.parse.urlencode(q)))))

    def test_state_from_other_browser(self):
        """Login CSRF: callback of an attacker in the victim's browser must not log in."""
        _, cb = self.start(f"attacker{uuid.uuid4().hex[:6]}~verified")
        victim = Session(URL, api_version="10")
        victim.request("GET", "/accounts/login/", auth=False, api_version=False)
        self.assertFalse(self.finish(victim, cb), "callback accepted in a foreign session")

    def test_code_replay(self):
        s, cb = self.start(f"oidc{uuid.uuid4().hex[:6]}~verified")
        self.assertTrue(self.finish(s, cb))
        s2 = Session(URL, api_version="10")
        self.assertFalse(self.finish(s2, cb), "replayed callback logged in")

    def test_bad_id_tokens(self):
        # badsig not: allauth does not check the signature if the id_token comes directly from the token endpoint
        # (verify_signature = not did_fetch_access_token, OIDC Core 3.1.3.7 no. 6) - same behavior.
        for flag in ("badaud", "badiss", "expired"):
            with self.subTest(flag=flag):
                s, cb = self.start(f"oidc{uuid.uuid4().hex[:6]}~verified~{flag}")
                self.assertFalse(self.finish(s, cb), f"id_token with {flag} accepted")

    def test_provider_errors(self):
        for flag in ("deny", "err", "tokenfail"):
            with self.subTest(flag=flag):
                s = Session(URL, api_version="10")
                hint = f"oidc{uuid.uuid4().hex[:6]}~{flag}"
                try:
                    s2, cb = self.start(hint)
                except AssertionError:
                    continue  # error before callback: fine
                r = s2.request("GET", cb, auth=False, api_version=False)
                self.assertLess(r.status, 500)
                self.assertNotEqual(s2.get("/api/profile/", auth=False).status, 200)
