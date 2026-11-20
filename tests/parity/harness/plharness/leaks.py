"""Leak scan for responses (body + header values).

Categories:
  password-hash   Django hasher formats (pbkdf2_sha256$..., argon2$..., bcrypt..., md5$...)
  token           any known user token (from the user catalogue)
  password        any known user password (>= 6 chars)
  secret          PAPERLESS_SECRET_KEY / SECRET_KEY values, private keys
  path            server filesystem paths (repo, reference data, media root, typical
                  install dirs, site-packages, Windows user dirs)
  stacktrace      Python tracebacks, Rust panics/backtraces, DB driver errors
  sql             raw SQL error fragments

A case may allow categories with ``"leak_allow": ["token"]`` (e.g. POST /api/token/ returns
the caller's own token; /api/logs/ legitimately contains paths). Leaks found in A (Django)
are reported as upstream findings; leaks in B fail the run.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass

HASH_RE = re.compile(
    r"\b(pbkdf2_sha256|pbkdf2_sha1|argon2(?:id|i|d)?|bcrypt_sha256|bcrypt|scrypt|md5|sha1|crypt)\$[A-Za-z0-9+/=$.,:-]{8,}"
)
STATIC_PATTERNS: list[tuple[str, re.Pattern]] = [
    ("password-hash", HASH_RE),
    ("secret", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----")),
    ("stacktrace", re.compile(r"Traceback \(most recent call last\)")),
    ("stacktrace", re.compile(r"File \\?\"[^\"]+\.py\\?\", line \d+")),
    ("stacktrace", re.compile(r"panicked at ")),
    ("stacktrace", re.compile(r"stack backtrace:|RUST_BACKTRACE")),
    ("stacktrace", re.compile(r"\b(?:django\.db\.utils|psycopg2?\.errors|sqlx::|sea_query::)\w*")),
    ("sql", re.compile(r"\b(?:syntax error at or near|relation \\?\"\w+\\?\" does not exist|SELECT .{1,80} FROM \\?\"?\w+)", re.I)),
    ("path", re.compile(r"[A-Za-z]:(?:\\\\|\\|/)(?:Users|Windows|Program Files|ProgramData)(?:\\\\|\\|/)", re.I)),
    # (?<![}\w]) : do not trigger in URL patterns like "/api/documents/{id}/root/" (false alarm in the OpenAPI schema)
    ("path", re.compile(r"(?<![}\w])/(?:usr/src/paperless|home/\w+/|root/|var/lib/|opt/paperless|tmp/paperless)")),
    ("path", re.compile(r"site-packages|\.cargo[\\/]registry")),
]


@dataclass
class Leak:
    category: str
    pattern: str
    excerpt: str

    def to_dict(self) -> dict:
        return {"category": self.category, "pattern": self.pattern, "excerpt": self.excerpt}


class LeakScanner:
    def __init__(self, secrets: list[str] | None = None, sensitive_paths: list[str] | None = None) -> None:
        self.dynamic: list[tuple[str, re.Pattern]] = []
        for s in secrets or []:
            if s and len(s) >= 6:
                cat = "token" if re.fullmatch(r"[0-9a-f]{40}", s) else "password"
                self.dynamic.append((cat, re.compile(re.escape(s))))
        for key in ("PAPERLESS_SECRET_KEY", "SECRET_KEY"):
            v = os.environ.get(key)
            if v and len(v) >= 8:
                self.dynamic.append(("secret", re.compile(re.escape(v))))
        for p in sensitive_paths or []:
            p = p.rstrip("\\/")
            if len(p) < 4:
                continue
            variants = {p, p.replace("\\", "/"), p.replace("\\", "\\\\"), p.replace("/", "\\"),
                        p.replace("/", "\\\\")}
            # Git-Bash style /c/Users/... as well
            m = re.match(r"^([A-Za-z]):[\\/](.*)$", p)
            if m:
                variants.add("/" + m.group(1).lower() + "/" + m.group(2).replace("\\", "/"))
            for v in variants:
                self.dynamic.append(("path", re.compile(re.escape(v), re.I)))

    def scan_text(self, text: str, allow: set[str] | None = None) -> list[Leak]:
        allow = allow or set()
        found: list[Leak] = []
        seen: set[tuple[str, str]] = set()
        for cat, pat in self.dynamic + STATIC_PATTERNS:
            if cat in allow:
                continue
            m = pat.search(text)
            if not m:
                continue
            key = (cat, pat.pattern)
            if key in seen:
                continue
            seen.add(key)
            start = max(0, m.start() - 40)
            excerpt = text[start : m.end() + 40].replace("\n", " ")
            # never copy a found secret verbatim into the report
            if cat in ("token", "password", "secret", "password-hash"):
                excerpt = excerpt.replace(m.group(0), f"<{cat}:{len(m.group(0))} chars>")
            found.append(Leak(cat, pat.pattern[:60], excerpt[:200]))
        return found

    def scan_response(self, resp, allow: set[str] | None = None) -> list[Leak]:
        text = resp.body[:5_000_000].decode("utf-8", errors="replace")
        hdr = "\n".join(f"{k}: {v}" for k, v in resp.headers if k.lower() not in ("set-cookie",))
        ct = resp.content_type
        # Binary payloads (PDF, images, zip) are not scanned for paths/hashes; their bytes
        # are compared for parity anyway.
        if ct and not (ct.startswith("text/") or "json" in ct or "xml" in ct or "javascript" in ct):
            text = ""
        return self.scan_text(text + "\n" + hdr, allow)
