"""Parity check pl-system: same request against Django and PaperRust, compare answers.

Read-only requests only (GET), so the reference data is never changed. Write routes
(tasks/acknowledge, tasks/run) must be compared inside a disposable copy of the DB.

Usage (from the repository root):

    python -I tests/parity/pl-system/parity.py \
        --django http://127.0.0.1:8000 --rust http://127.0.0.1:8001 \
        --token <admin token> [--token-user <token of a non-staff user>]

Exit code 0 = no differences. Volatile values (server time based, mtimes, ping) are
normalised as listed in VOLATILE.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request

PATHS = [
    "/api/logs/",
    "/api/logs/paperless/",
    "/api/logs/paperless/?limit=5",
    "/api/logs/paperless/?limit=abc",
    "/api/logs/bogus/",
    "/api/statistics/",
    "/api/status/",
    "/api/config/",
    "/api/config/1/",
    "/api/config/999/",
    "/api/config/abc/",
    "/api/tasks/",
    "/api/tasks/?page=2&page_size=3",
    "/api/tasks/?ordering=status,-date_done",
    "/api/tasks/?status=success&status=failure",
    "/api/tasks/?status=bogus",
    "/api/tasks/?is_complete=true",
    "/api/tasks/?acknowledged=false",
    "/api/tasks/?name=consume",
    "/api/tasks/?result=duplicate",
    "/api/tasks/?owner=abc",
    "/api/tasks/?date_created_after=2020-01-01",
    "/api/tasks/summary/",
    "/api/tasks/summary/?days=abc",
    "/api/tasks/status_counts/",
    "/api/tasks/active/",
    "/api/tasks/1/",
    "/api/tasks/999999/",
]

# (path prefix, json path) pairs whose values depend on time/host state
VOLATILE = {
    "/api/status/": [
        ("server_os",),
        ("storage", "available"),
        ("tasks", "celery_url"),
        ("tasks", "index_last_modified"),
    ],
}


def fetch(base: str, path: str, token: str, version: str | None) -> tuple[int, object]:
    accept = "application/json" + (f"; version={version}" if version else "")
    req = urllib.request.Request(base + path, headers={"Authorization": f"Token {token}", "Accept": accept})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            body = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        body = e.read()
        status = e.code
    try:
        return status, json.loads(body or b"null")
    except ValueError:
        return status, body.decode("utf-8", "replace")


def normalise(path: str, data: object, base: str) -> object:
    text = json.dumps(data, sort_keys=False).replace(base, "<BASE>")
    data = json.loads(text)
    for prefix, paths in VOLATILE.items():
        if path.startswith(prefix) and isinstance(data, dict):
            for p in paths:
                cur = data
                for k in p[:-1]:
                    cur = cur.get(k, {}) if isinstance(cur, dict) else {}
                if isinstance(cur, dict) and p[-1] in cur:
                    cur[p[-1]] = "<volatile>"
    return data


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--django", required=True)
    ap.add_argument("--rust", required=True)
    ap.add_argument("--token", required=True)
    ap.add_argument("--token-user")
    args = ap.parse_args()
    tokens = [("admin", args.token)] + ([("user", args.token_user)] if args.token_user else [])
    different = 0
    total = 0
    for who, tok in tokens:
        for version in (None, "9"):
            for path in PATHS:
                total += 1
                ds, dj = fetch(args.django, path, tok, version)
                rs, ru = fetch(args.rust, path, tok, version)
                dj = normalise(path, dj, args.django)
                ru = normalise(path, ru, args.rust)
                same = ds == rs and json.dumps(dj) == json.dumps(ru)
                if not same:
                    different += 1
                    print(f"DIFF [{who} v{version or 10}] {path}: django={ds} rust={rs}")
                    print("  django:", json.dumps(dj)[:400])
                    print("  rust:  ", json.dumps(ru)[:400])
    print(f"total={total} different={different}")
    return 1 if different else 0


if __name__ == "__main__":
    sys.exit(main())
