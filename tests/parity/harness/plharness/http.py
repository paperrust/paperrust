"""Loopback-only HTTP client for the parity harness and the security suite.

* Every target URL is checked: only 127.0.0.0/8, ::1 and names resolving exclusively
  to loopback addresses are allowed (``NonLoopbackTarget`` otherwise). This also applies
  to redirects followed by the client.
* No Accept-Encoding is sent (Django has compression middleware; we want raw bodies).
* Auth modes: anonymous, token (``Authorization: Token``), basic, session (allauth form
  login with CSRF). For session auth every unsafe request carries ``X-CSRFToken``
  unless ``csrf=False`` is passed (used by the CSRF tests).
"""

from __future__ import annotations

import base64
import http.client
import ipaddress
import json
import re
import socket
import time
from dataclasses import dataclass, field
from http.cookies import SimpleCookie
from typing import Any, Iterable
from urllib.parse import quote, urlencode, urljoin, urlsplit

SAFE_METHODS = {"GET", "HEAD", "OPTIONS", "TRACE"}


class NonLoopbackTarget(ValueError):
    """Raised when a request would leave the local machine."""


def _is_loopback_host(host: str) -> bool:
    host = host.strip("[]")
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        pass
    try:
        infos = socket.getaddrinfo(host, None)
    except OSError:
        return False
    addrs = {info[4][0] for info in infos}
    if not addrs:
        return False
    for addr in addrs:
        try:
            if not ipaddress.ip_address(addr.split("%")[0]).is_loopback:
                return False
        except ValueError:
            return False
    return True


def assert_loopback(url: str) -> None:
    parts = urlsplit(url)
    if parts.scheme not in ("http", "https"):
        raise NonLoopbackTarget(f"unsupported scheme in {url!r}")
    if not parts.hostname or not _is_loopback_host(parts.hostname):
        raise NonLoopbackTarget(f"refusing non-loopback target {url!r}")


def encode_query(query: Any) -> str:
    """Encode query params like Angular's HttpParams (bool -> 'true'/'false')."""
    if query is None:
        return ""
    if isinstance(query, str):
        return query.lstrip("?")
    pairs: list[tuple[str, str]] = []
    items: Iterable = query.items() if isinstance(query, dict) else query
    for key, value in items:
        values = value if isinstance(value, (list, tuple)) else [value]
        for v in values:
            if v is None:
                continue
            if isinstance(v, bool):
                v = "true" if v else "false"
            pairs.append((str(key), str(v)))
    return urlencode(pairs, quote_via=quote, safe=",")


@dataclass
class Response:
    status: int
    reason: str
    headers: list[tuple[str, str]]
    body: bytes
    elapsed_ms: float
    method: str = "GET"
    url: str = ""
    _json: Any = field(default=None, repr=False)
    _json_parsed: bool = field(default=False, repr=False)

    def header(self, name: str, default: str | None = None) -> str | None:
        name = name.lower()
        for k, v in self.headers:
            if k.lower() == name:
                return v
        return default

    def header_all(self, name: str) -> list[str]:
        name = name.lower()
        return [v for k, v in self.headers if k.lower() == name]

    @property
    def content_type(self) -> str:
        return (self.header("content-type") or "").split(";")[0].strip().lower()

    @property
    def is_json(self) -> bool:
        ct = self.content_type
        return ct == "application/json" or ct.endswith("+json")

    @property
    def text(self) -> str:
        return self.body.decode("utf-8", errors="replace")

    def json(self) -> Any:
        if not self._json_parsed:
            self._json_parsed = True
            try:
                self._json = json.loads(self.body.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                self._json = None
        return self._json


class Session:
    """A cookie-keeping client bound to one base URL."""

    def __init__(
        self,
        base_url: str,
        *,
        host_header: str | None = None,
        timeout: float = 60.0,
        auth: tuple | None = None,
        api_version: str | None = None,
    ) -> None:
        assert_loopback(base_url)
        self.base_url = base_url.rstrip("/")
        self.host_header = host_header
        self.timeout = timeout
        self.auth = auth  # None | ("token", t) | ("basic", u, p) | ("session",)
        self.api_version = api_version
        self.cookies: dict[str, str] = {}

    # ------------------------------------------------------------------ cookies
    def _store_cookies(self, resp: Response) -> None:
        for raw in resp.header_all("set-cookie"):
            jar = SimpleCookie()
            try:
                jar.load(raw)
            except Exception:  # malformed cookie: ignore like a browser would
                continue
            for name, morsel in jar.items():
                expired = morsel["max-age"] == "0" or (
                    morsel["expires"] and "1970" in morsel["expires"]
                )
                if expired or morsel.value == "":
                    self.cookies.pop(name, None)
                else:
                    self.cookies[name] = morsel.value

    def csrf_cookie(self) -> str | None:
        """Value of the CSRF cookie (honours PAPERLESS_COOKIE_PREFIX: any name ending in csrftoken)."""
        for k, v in self.cookies.items():
            if k.endswith("csrftoken"):
                return v
        return None

    def has_session(self) -> bool:
        return any(k.endswith("sessionid") for k in self.cookies)

    # ------------------------------------------------------------------ request
    def request(
        self,
        method: str,
        path: str,
        *,
        query: Any = None,
        json_body: Any = None,
        form: dict | None = None,
        data: bytes | None = None,
        content_type: str | None = None,
        headers: dict[str, str] | None = None,
        follow: bool = False,
        csrf: bool = True,
        auth: tuple | None | bool = True,
        api_version: str | None | bool = True,
        max_redirects: int = 5,
    ) -> Response:
        method = method.upper()
        qs = encode_query(query)
        if path.startswith("http://") or path.startswith("https://"):
            url = path
        else:
            url = self.base_url + (path if path.startswith("/") else "/" + path)
        if qs:
            url += ("&" if "?" in url else "?") + qs
        assert_loopback(url)

        hdrs: dict[str, str] = {}
        if self.host_header:
            hdrs["Host"] = self.host_header
        ver = self.api_version if api_version is True else api_version
        if ver is not None and ver is not False:
            if ver == "none":
                hdrs["Accept"] = "application/json"
            else:
                hdrs["Accept"] = f"application/json; version={ver}"
        body: bytes | None = None
        if json_body is not None:
            body = json.dumps(json_body).encode("utf-8")
            hdrs["Content-Type"] = "application/json"
        elif form is not None:
            body = urlencode(form, doseq=True).encode("utf-8")
            hdrs["Content-Type"] = "application/x-www-form-urlencoded"
        elif data is not None:
            body = data
        if content_type:
            hdrs["Content-Type"] = content_type

        mode = self.auth if auth is True else (auth or None)
        if mode:
            if mode[0] == "token":
                hdrs["Authorization"] = f"Token {mode[1]}"
            elif mode[0] == "basic":
                cred = base64.b64encode(f"{mode[1]}:{mode[2]}".encode()).decode()
                hdrs["Authorization"] = f"Basic {cred}"
            elif mode[0] == "raw":
                hdrs["Authorization"] = mode[1]
        if self.cookies:
            hdrs["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
        csrf_value = self.csrf_cookie()
        if csrf and method not in SAFE_METHODS and csrf_value:
            hdrs["X-CSRFToken"] = csrf_value
        if headers:
            hdrs.update(headers)

        resp = self._send(method, url, body, hdrs)
        self._store_cookies(resp)
        redirects = 0
        while follow and resp.status in (301, 302, 303, 307, 308) and redirects < max_redirects:
            loc = resp.header("location")
            if not loc:
                break
            nxt = urljoin(url, loc)
            # A redirect pointing at the shared Host header is mapped back to our target.
            p = urlsplit(nxt)
            if self.host_header and p.netloc == self.host_header:
                nxt = self.base_url + p.path + (f"?{p.query}" if p.query else "")
            assert_loopback(nxt)
            redirects += 1
            m = method if resp.status in (307, 308) else "GET"
            b = body if resp.status in (307, 308) else None
            h = {k: v for k, v in hdrs.items() if k not in ("Content-Type",) or b is not None}
            if self.cookies:
                h["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
            url = nxt
            resp = self._send(m, url, b, h)
            self._store_cookies(resp)
        return resp

    def _send(self, method: str, url: str, body: bytes | None, hdrs: dict[str, str]) -> Response:
        parts = urlsplit(url)
        conn_cls = http.client.HTTPSConnection if parts.scheme == "https" else http.client.HTTPConnection
        conn = conn_cls(parts.hostname, parts.port, timeout=self.timeout)
        target = parts.path or "/"
        if parts.query:
            target += "?" + parts.query
        t0 = time.perf_counter()
        try:
            conn.putrequest(method, target, skip_host=True, skip_accept_encoding=True)
            if "Host" not in hdrs:
                conn.putheader("Host", parts.netloc)
            for k, v in hdrs.items():
                conn.putheader(k, v)
            if body is not None:
                conn.putheader("Content-Length", str(len(body)))
            conn.endheaders(body)
            r = conn.getresponse()
            payload = r.read()
            elapsed = (time.perf_counter() - t0) * 1000
            return Response(
                status=r.status,
                reason=r.reason,
                headers=list(r.getheaders()),
                body=payload,
                elapsed_ms=elapsed,
                method=method,
                url=url,
            )
        finally:
            conn.close()

    # convenience
    def get(self, path: str, **kw) -> Response:
        return self.request("GET", path, **kw)

    def post(self, path: str, **kw) -> Response:
        return self.request("POST", path, **kw)


_CSRF_INPUT = re.compile(r'name=["\']csrfmiddlewaretoken["\']\s+value=["\']([^"\']+)["\']')
_CSRF_INPUT2 = re.compile(r'value=["\']([^"\']+)["\']\s+name=["\']csrfmiddlewaretoken["\']')


def session_login(sess: Session, username: str, password: str, login_path: str = "/accounts/login/") -> bool:
    """Log in like a browser: GET the allauth form (csrftoken cookie + hidden field), POST it.

    Returns True when a ``sessionid`` cookie was obtained. Sets ``sess.auth = ("session",)``.
    """
    r = sess.request("GET", login_path, auth=False, api_version=False)
    m = _CSRF_INPUT.search(r.text) or _CSRF_INPUT2.search(r.text)
    token = m.group(1) if m else (sess.csrf_cookie() or "")
    referer_host = sess.host_header or urlsplit(sess.base_url).netloc
    r = sess.request(
        "POST",
        login_path,
        form={"login": username, "password": password, "csrfmiddlewaretoken": token},
        headers={"Referer": f"http://{referer_host}{login_path}"},
        auth=False,
        api_version=False,
        csrf=False,
    )
    ok = sess.has_session()
    if ok:
        sess.auth = ("session",)
    return ok
