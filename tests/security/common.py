"""Shared helpers of the security suite.

TARGET=django (default URL http://127.0.0.1:8000) or TARGET=rust (http://127.0.0.1:8080);
SECURITY_URL overrides the URL. Users come from the same catalogue as the parity harness
(reference/users.json, reference/README.md or PARITY_USERS_FILE). Only loopback targets.
"""

from __future__ import annotations

import os
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(REPO / "tests" / "parity" / "harness"))

from plharness import users as users_mod  # noqa: E402
from plharness.config import REFERENCE_DIR  # noqa: E402
from plharness.http import Response, Session, session_login  # noqa: E402
from plharness.leaks import LeakScanner  # noqa: E402

TARGET = os.environ.get("TARGET", "django").lower()
DEFAULT_URLS = {
    "django": os.environ.get("PARITY_A_URL", "http://127.0.0.1:8000"),
    "rust": os.environ.get("PARITY_B_URL", "http://127.0.0.1:8080"),
}
if TARGET not in DEFAULT_URLS and not os.environ.get("SECURITY_URL"):
    raise SystemExit(f"TARGET must be django or rust, got {TARGET!r}")
BASE_URL = (os.environ.get("SECURITY_URL") or DEFAULT_URLS[TARGET]).rstrip("/")
HOST_HEADER = os.environ.get("PARITY_HOST_HEADER") or None
USERS = users_mod.load(os.environ.get("PARITY_USERS_FILE") or None)
SCANNER = LeakScanner(USERS.secrets(), [str(REPO), str(REFERENCE_DIR)] + [
    os.environ[v] for v in ("PAPERLESS_MEDIA_ROOT", "PAPERLESS_DATA_DIR") if os.environ.get(v)])
RESET_SCRIPT = REFERENCE_DIR / "reset.sh"
DENIED = "You do not have permission to perform this action."
NOT_PROVIDED = "Authentication credentials were not provided."


def session(user: str | None = None, api_version: str = "10") -> Session:
    """Token session for a catalogue user; ``None``/``"anon"`` -> anonymous."""
    s = Session(BASE_URL, host_header=HOST_HEADER, api_version=api_version)
    if user and user != "anon":
        u = USERS.get(user)
        if u is None or not u.token:
            raise unittest.SkipTest(f"user {user!r} with token not in catalogue ({USERS.source})")
        s.auth = ("token", u.token)
    return s


def login_session(user: str) -> Session:
    u = USERS.get(user)
    if u is None or not u.username or not u.password:
        raise unittest.SkipTest(f"user {user!r} with password not in catalogue")
    s = Session(BASE_URL, host_header=HOST_HEADER, api_version="10")
    if not session_login(s, u.username, u.password):
        raise AssertionError(f"session login failed for {user}")
    return s


def need_user(name: str):
    u = USERS.get(name)
    if u is None:
        raise unittest.SkipTest(f"user {name!r} not in catalogue ({USERS.source})")
    return u


def assert_no_leak(tc: unittest.TestCase, r: Response, allow: set[str] | None = None, what: str = "") -> None:
    leaks = SCANNER.scan_response(r, allow)
    tc.assertEqual([], [lk.to_dict() for lk in leaks], f"leak in {what or r.url} (HTTP {r.status})")


def detail(r: Response) -> str | None:
    j = r.json()
    return j.get("detail") if isinstance(j, dict) else None


class TargetCase(unittest.TestCase):
    """Base class: skips everything when the target is not reachable."""

    @classmethod
    def setUpClass(cls):
        try:
            Session(BASE_URL, host_header=HOST_HEADER, timeout=5).get("/api/")
        except OSError as e:
            raise unittest.SkipTest(f"target {BASE_URL} unreachable: {e}")
