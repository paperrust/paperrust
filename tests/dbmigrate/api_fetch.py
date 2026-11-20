"""Fetches a fixed selection of GET responses from a running paperless-ngx (final test of the DB move).

Invocation: python -I api_fetch.py <base-url> <out.json>
stdlib only, loopback only. Tokens from reference/seed.py (fixed local test values).
"""

from __future__ import annotations

import json
import re
import sys
import urllib.error
import urllib.request

TOKENS = {
    "admin": "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001",
    "alice": "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002",
    "bob": "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003",
}

COMMON = [
    "/api/documents/?ordering=id&page_size=100000",
    "/api/documents/?ordering=-added&page_size=25",
    "/api/documents/?tags__id__all=1&ordering=id",
    "/api/documents/?is_tagged=0&ordering=id",
    "/api/documents/?correspondent__isnull=1&ordering=id",
    "/api/documents/?created__date__gte=2020-01-01&ordering=id",
    "/api/documents/?query=invoice&ordering=id",
    "/api/documents/?custom_fields__icontains=a&ordering=id",
    "/api/documents/next_asn/",
    "/api/tags/?page_size=1000&ordering=id",
    "/api/correspondents/?page_size=1000&ordering=id",
    "/api/document_types/?page_size=1000&ordering=id",
    "/api/storage_paths/?page_size=1000&ordering=id",
    "/api/custom_fields/?page_size=1000&ordering=id",
    "/api/saved_views/?page_size=1000",
    "/api/share_links/?page_size=1000",
    "/api/workflows/?page_size=1000",
    "/api/mail_accounts/?page_size=1000",
    "/api/mail_rules/?page_size=1000",
    "/api/processed_mail/?page_size=1000",
    "/api/users/?page_size=1000",
    "/api/groups/?page_size=1000",
    "/api/ui_settings/",
    "/api/profile/",
    "/api/tasks/",
    "/api/statistics/",
    "/api/config/",
    "/api/trash/?page_size=1000",
    "/api/search/?query=invoice",
    "/api/search/autocomplete/?term=in",
]

# Responses that intentionally differ between backends (engine name, DB size …)
BACKEND_SPECIFIC = {"/api/status/"}

# Sorting/text comparison depends on the backend (collation: SQLite binary + ASCII LIKE vs.
# PostgreSQL ICU; NULL sorting: SQLite/MySQL NULL first, PostgreSQL NULL last) – this is
# Django backend behavior, not a move error. For page fetches this changes the page contents.
COLLATION = [
    "/api/documents/?ordering=archive_serial_number&page_size=25&page=2",
    "/api/documents/?ordering=title&page_size=100000",
    "/api/tags/?ordering=name&page_size=1000",
    "/api/documents/?title__icontains=%C3%BC&ordering=id",
]


def get(base: str, path: str, token: str):
    req = urllib.request.Request(base + path, headers={"Authorization": f"Token {token}", "Accept": "application/json; version=10"})
    try:
        with urllib.request.urlopen(req, timeout=120) as r:
            status, body = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, body = e.code, e.read()
    text = body.decode("utf-8", "replace")
    text = re.sub(r"http://127\.0\.0\.1:\d+", "http://HOST", text)
    try:
        return {"status": status, "json": json.loads(text)}
    except ValueError:
        return {"status": status, "text": text[:2000]}


def main() -> None:
    base, out = sys.argv[1].rstrip("/"), sys.argv[2]
    res = {}
    for user, tok in TOKENS.items():
        paths = list(COMMON)
        docs = get(base, "/api/documents/?ordering=id&page_size=100000&fields=id,title", tok)
        ids = [d["id"] for d in docs.get("json", {}).get("results", [])]
        edge = [d["id"] for d in docs.get("json", {}).get("results", []) if d["title"].startswith("Ünïcødé")]
        for i in (ids[:3] + edge[:1]):
            paths += [f"/api/documents/{i}/", f"/api/documents/{i}/notes/", f"/api/documents/{i}/history/", f"/api/documents/{i}/metadata/"]
        for p in paths + COLLATION + sorted(BACKEND_SPECIFIC):
            res[f"{user} {p}"] = get(base, p, tok)
    with open(out, "w", encoding="utf-8") as f:
        json.dump(res, f, ensure_ascii=False, indent=1, sort_keys=True)
    print(f"{len(res)} Antworten von {base} -> {out}")


if __name__ == "__main__":
    main()
