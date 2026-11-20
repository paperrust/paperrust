"""pytest plugin: run upstream's DRF API tests over HTTP against a live server (PaperRust).

Upstream code stays untouched. Load with ``-p paperless_http_adapter`` (this directory on
``PYTHONPATH``). Without ``UPSTREAM_ADAPTER_TARGET`` the plugin is inert (baseline run).

How it works
------------
1. **Visible test data.** ``django.test.TestCase`` normally wraps each test in a
   transaction that is rolled back - a second process (the live server) would never see
   the rows. The plugin makes ``TestCase._databases_support_transactions`` return False,
   which is Django's own code path for non-transactional backends: ``setUpTestData`` runs
   per test, data is COMMITTED, and ``TransactionTestCase._fixture_teardown`` flushes all
   tables afterwards. pytest-django's ``db`` fixture uses the same TestCase class, so
   function-style tests behave the same.
2. **Requests over HTTP.** ``django.test.Client.request`` (also used by DRF's
   ``APIClient``) is replaced: the WSGI environ that Django builds is turned into a real
   HTTP request to ``UPSTREAM_ADAPTER_TARGET`` for paths under
   ``UPSTREAM_ADAPTER_PATHS`` (default ``/api/,/share/,/logo/,/fetch/``); other paths stay
   in-process. ``Host: testserver`` is sent, so absolute URLs in responses match the
   upstream assertions.
3. **Authentication.** ``force_authenticate(user)`` cannot cross a process boundary; the
   plugin sends ``Authorization: Token <key>`` for that user (DRF ``Token`` row created on
   demand, committed). ``force_authenticate(token=t)`` sends ``t.key``.
   ``client.force_login``/``login`` create a real session row + cookie, which is sent as is
   (target must use the same SECRET_KEY). For session-authenticated unsafe requests a
   CSRF cookie/header pair is added unless the client was built with
   ``enforce_csrf_checks=True`` (Django's test client skips CSRF the same way).
4. **Response.** A ``django.http.HttpResponse`` with status, headers, body and cookies is
   returned; ``.data`` is the parsed JSON (DRF test responses expose ``.data``),
   ``.json()``, ``.url`` (redirects), ``.streaming_content``, ``.client``, ``.request`` are set.

Environment
-----------
UPSTREAM_ADAPTER_TARGET   e.g. http://127.0.0.1:8080 (loopback only, enforced)
UPSTREAM_ADAPTER_PATHS    comma-separated path prefixes to redirect
UPSTREAM_ADAPTER_HOST     Host header (default "testserver")
UPSTREAM_ADAPTER_TIMEOUT  seconds (default 60)
UPSTREAM_ADAPTER_LOG      file to append one JSON line per redirected request

Limits: see docs/contract/testing.md ("Upstream-Test-Adapter").
"""

from __future__ import annotations

import http.client
import ipaddress
import json
import os
import secrets
import socket
import time
from functools import partial
from urllib.parse import quote, urlsplit

import pytest

_TARGET = os.environ.get("UPSTREAM_ADAPTER_TARGET", "").rstrip("/")
_PATHS = tuple(p for p in os.environ.get("UPSTREAM_ADAPTER_PATHS", "/api/,/share/,/logo/,/fetch/").split(",") if p)
_HOST = os.environ.get("UPSTREAM_ADAPTER_HOST", "testserver")
_TIMEOUT = float(os.environ.get("UPSTREAM_ADAPTER_TIMEOUT", "60"))
_LOG = os.environ.get("UPSTREAM_ADAPTER_LOG")
_STATS = {"redirected": 0, "in_process": 0, "tokens_created": 0}
_HOP_BY_HOP = {"connection", "keep-alive", "transfer-encoding", "content-length", "set-cookie", "server", "date"}


def _assert_loopback(url: str) -> None:
    parts = urlsplit(url)
    host = (parts.hostname or "").strip("[]")
    try:
        ok = ipaddress.ip_address(host).is_loopback
    except ValueError:
        try:
            ok = all(ipaddress.ip_address(i[4][0].split("%")[0]).is_loopback for i in socket.getaddrinfo(host, None))
        except OSError:
            ok = False
    if parts.scheme != "http" or not ok:
        raise pytest.UsageError(f"UPSTREAM_ADAPTER_TARGET must be an http loopback URL, got {url!r}")


def _token_for(user) -> str:
    from rest_framework.authtoken.models import Token

    tok, created = Token.objects.get_or_create(user_id=user.pk)
    if created:
        _STATS["tokens_created"] += 1
    return tok.key


def _environ_to_request(environ: dict) -> tuple[str, str, dict[str, str], bytes]:
    method = environ.get("REQUEST_METHOD", "GET").upper()
    path = environ.get("PATH_INFO", "/")
    raw_path = quote(path.encode("iso-8859-1"), safe="/:@!$&'()*+,;=-._~")
    qs = environ.get("QUERY_STRING", "")
    target = raw_path + (f"?{qs}" if qs else "")
    headers: dict[str, str] = {}
    for k, v in environ.items():
        if k.startswith("HTTP_") and isinstance(v, str):
            name = "-".join(p.capitalize() for p in k[5:].split("_"))
            headers[name] = v
    if environ.get("CONTENT_TYPE"):
        headers["Content-Type"] = environ["CONTENT_TYPE"]
    if not headers.get("Cookie"):
        headers.pop("Cookie", None)
    headers["Host"] = environ.get("HTTP_HOST") or _HOST
    body = b""
    length = int(environ.get("CONTENT_LENGTH") or 0)
    stream = environ.get("wsgi.input")
    if stream is not None and length:
        body = stream.read(length)
    return method, target, headers, body


def _send(method: str, target: str, headers: dict[str, str], body: bytes):
    parts = urlsplit(_TARGET)
    conn = http.client.HTTPConnection(parts.hostname, parts.port, timeout=_TIMEOUT)
    try:
        conn.putrequest(method, target, skip_host=True, skip_accept_encoding=True)
        for k, v in headers.items():
            conn.putheader(k, v)
        if body or method in ("POST", "PUT", "PATCH"):
            conn.putheader("Content-Length", str(len(body)))
        conn.endheaders(body or None)
        r = conn.getresponse()
        return r.status, r.reason, r.getheaders(), r.read()
    finally:
        conn.close()


def _build_response(client, request: dict, status, reason, raw_headers, payload):
    from django.http import HttpResponse

    resp = HttpResponse(content=payload, status=status, reason=reason)
    for k in list(resp.headers.keys()):
        del resp.headers[k]
    for k, v in raw_headers:
        lk = k.lower()
        if lk == "set-cookie":
            resp.cookies.load(v)
            continue
        if lk in _HOP_BY_HOP:
            continue
        resp.headers[k] = f"{resp.headers[k]}, {v}" if k in resp.headers else v
    ctype = (resp.headers.get("Content-Type") or "").split(";")[0].strip().lower()
    if ctype == "application/json" or ctype.endswith("+json"):
        try:
            resp.data = json.loads(payload.decode("utf-8")) if payload else None
        except (ValueError, UnicodeDecodeError):
            pass
    if 300 <= status < 400 and resp.headers.get("Location"):
        resp.url = resp.headers["Location"]
    resp.streaming_content = iter([payload])
    resp.client = client
    resp.request = request
    resp.templates = []
    resp.context = None
    resp.wsgi_request = None
    resp.json = partial(client._parse_json, resp)
    resp.adapter_target = _TARGET
    if resp.cookies:
        client.cookies.update(resp.cookies)
    return resp


def _install() -> None:
    from django.test import Client, TestCase

    # 1) committed test data (Django's non-transactional TestCase path)
    TestCase._databases_support_transactions = classmethod(lambda cls: False)

    original = Client.request

    def request(self, **req):
        environ = self._base_environ(**req)
        path = environ.get("PATH_INFO", "/")
        if not path.startswith(_PATHS):
            _STATS["in_process"] += 1
            return original(self, **req)
        method, target, headers, body = _environ_to_request(environ)
        handler = getattr(self, "handler", None)
        force_user = getattr(handler, "_force_user", None)
        force_token = getattr(handler, "_force_token", None)
        if "Authorization" not in headers:
            if force_token is not None:
                headers["Authorization"] = f"Token {getattr(force_token, 'key', force_token)}"
            elif force_user is not None:
                headers["Authorization"] = f"Token {_token_for(force_user)}"
        cookie = headers.get("Cookie", "")
        enforce = getattr(handler, "enforce_csrf_checks", False)
        from django.conf import settings

        if (not enforce and method not in ("GET", "HEAD", "OPTIONS", "TRACE")
                and f"{settings.SESSION_COOKIE_NAME}=" in cookie and "X-Csrftoken" not in headers):
            tok = secrets.token_hex(16)  # 32 chars, Django accepts unmasked secrets
            headers["Cookie"] = f"{cookie}; {settings.CSRF_COOKIE_NAME}={tok}"
            headers["X-Csrftoken"] = tok
            headers.setdefault("Referer", f"http://{_HOST}/")
        t0 = time.perf_counter()
        status, reason, raw_headers, payload = _send(method, target, headers, body)
        _STATS["redirected"] += 1
        if _LOG:
            with open(_LOG, "a", encoding="utf-8") as f:
                f.write(json.dumps({"method": method, "target": target, "status": status,
                                    "ms": round((time.perf_counter() - t0) * 1000, 1)}) + "\n")
        return _build_response(self, req, status, reason, raw_headers, payload)

    Client.request = request


def pytest_addoption(parser):
    parser.addoption("--adapter-target", default=None, help="override UPSTREAM_ADAPTER_TARGET")


@pytest.hookimpl(trylast=True)
def pytest_configure(config):
    global _TARGET
    _TARGET = (config.getoption("--adapter-target") or _TARGET).rstrip("/")
    if not _TARGET:
        return
    _assert_loopback(_TARGET)
    if getattr(config.option, "numprocesses", None) not in (None, 0, "0"):
        raise pytest.UsageError("paperless_http_adapter needs a single process: pass -n 0 / -p no:xdist")
    config.addinivalue_line("markers", "adapter_skip: test cannot run over HTTP (adapter limitation)")


@pytest.hookimpl(trylast=True)
def pytest_sessionstart(session):
    if _TARGET:
        _install()


def pytest_report_header(config):
    if _TARGET:
        return f"paperless_http_adapter: {', '.join(_PATHS)} -> {_TARGET} (Host: {_HOST})"
    return "paperless_http_adapter: inactive (baseline, in-process)"


def pytest_terminal_summary(terminalreporter):
    if _TARGET:
        terminalreporter.write_line(
            f"paperless_http_adapter: {_STATS['redirected']} requests over HTTP, "
            f"{_STATS['in_process']} in-process, {_STATS['tokens_created']} tokens created")
