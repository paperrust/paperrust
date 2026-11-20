"""Rate limit of the token login (POST /api/token/, DRF ScopedRateThrottle scope "login",
rate PAPERLESS_TOKEN_THROTTLE_RATE, default 5/min, keyed by client IP).

Runs last (file name) because it locks token logins from 127.0.0.1 for up to one minute.
Skip with SECURITY_SKIP_RATELIMIT=1. Expected rate: SECURITY_TOKEN_RATE (default "5/min").
"""

from __future__ import annotations

import os
import unittest

from common import TargetCase, detail, session

RATE = os.environ.get("SECURITY_TOKEN_RATE", "5/min")


@unittest.skipIf(os.environ.get("SECURITY_SKIP_RATELIMIT"), "SECURITY_SKIP_RATELIMIT set")
class TokenRateLimit(TargetCase):
    def test_token_login_throttled(self):
        n = int(RATE.split("/")[0])
        if n > 50:
            self.skipTest(f"rate {RATE} too high to probe")
        statuses = []
        last = None
        for _ in range(n + 2):
            last = session().post("/api/token/", json_body={"username": "rate-limit-probe", "password": "wrong"})
            statuses.append(last.status)
            if last.status == 429:
                break
        self.assertIn(429, statuses, f"no throttling after {len(statuses)} attempts: {statuses}")
        self.assertTrue(all(s == 400 for s in statuses[:-1]), statuses)
        self.assertLessEqual(statuses.index(429), n, statuses)
        self.assertTrue((detail(last) or "").startswith("Request was throttled."), detail(last))
        self.assertIsNotNone(last.header("retry-after"))

    def test_wrong_credentials_message(self):
        # independent of throttling state: either 400 with DRF text or already 429
        r = session().post("/api/token/", json_body={"username": "x", "password": "y"})
        self.assertIn(r.status, (400, 429))
        if r.status == 400:
            self.assertEqual(r.json(), {"non_field_errors": ["Unable to log in with provided credentials."]})
