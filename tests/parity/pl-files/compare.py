#!/usr/bin/env python3
"""Parity check for pl-files: Django vs. Rust, read-only.

Usage:
    python -I compare.py --django http://127.0.0.1:8000 --rust http://127.0.0.1:8080 \
        --auth "Token <key>" --doc 1 --doc 2 [--share SLUG] [--bulk 1,2]

Compares status code, a fixed set of headers and the SHA-256 of the body for
download/preview/thumb (with original/version/follow_formatting variants and
conditional requests), /share/<slug>, /logo/, /favicon.ico and bulk_download
(ZIP entry names + per-entry content hashes instead of raw bytes, see
docs/contract/pl-files.md "Known differences" 3). Only stdlib; never writes.
"""

import argparse
import hashlib
import io
import json
import sys
import urllib.error
import urllib.request
import zipfile

HEADERS = [
    "Content-Type",
    "Content-Disposition",
    "Content-Length",
    "ETag",
    "Last-Modified",
    "Cache-Control",
    "Allow",
    "Location",
    "WWW-Authenticate",
]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *a, **k):
        return None


OPENER = urllib.request.build_opener(NoRedirect)


def fetch(base, path, auth=None, method="GET", body=None, extra=None):
    req = urllib.request.Request(base + path, method=method, data=body)
    if auth:
        req.add_header("Authorization", auth)
    for k, v in (extra or {}).items():
        req.add_header(k, v)
    try:
        resp = OPENER.open(req, timeout=60)
    except urllib.error.HTTPError as e:
        resp = e
    data = resp.read()
    hdrs = {h: resp.headers.get(h) for h in HEADERS}
    return resp.status if hasattr(resp, "status") else resp.code, hdrs, data


def summarize_zip(data):
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            return [
                (i.filename, i.compress_type, hashlib.sha256(z.read(i)).hexdigest())
                for i in z.infolist()
            ]
    except zipfile.BadZipFile:
        return None


def compare(name, a, b, zip_body=False):
    sa, ha, da = a
    sb, hb, db = b
    diffs = []
    if sa != sb:
        diffs.append(f"status {sa} != {sb}")
    for h in HEADERS:
        if zip_body and h == "Content-Length":
            continue
        if ha.get(h) != hb.get(h):
            diffs.append(f"{h}: {ha.get(h)!r} != {hb.get(h)!r}")
    if zip_body and sa == 200:
        if summarize_zip(da) != summarize_zip(db):
            diffs.append("zip entries differ")
    elif hashlib.sha256(da).digest() != hashlib.sha256(db).digest():
        diffs.append(f"body differs ({len(da)} vs {len(db)} bytes)")
    print(("SAME " if not diffs else "DIFF ") + name)
    for d in diffs:
        print("     " + d)
    return not diffs


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--django", required=True)
    p.add_argument("--rust", required=True)
    p.add_argument("--auth")
    p.add_argument("--doc", action="append", type=int, default=[])
    p.add_argument("--share", action="append", default=[])
    p.add_argument("--bulk", action="append", default=[])
    args = p.parse_args()

    ok = True
    cases = []
    for d in args.doc:
        for ep in ("download", "preview", "thumb"):
            for q in ("", "?original=true", "?follow_formatting=true", "?version=abc", "?version=999999"):
                cases.append((f"/api/documents/{d}/{ep}/{q}", None))
        cases.append((f"/api/documents/{d}/download/?original=true&follow_formatting=1", None))
    cases.append(("/api/documents/999999/download/", None))
    for path, extra in cases:
        a = fetch(args.django, path, args.auth, extra=extra)
        b = fetch(args.rust, path, args.auth, extra=extra)
        ok &= compare(path, a, b)
        # conditional replay with the validators Django returned
        if a[1].get("ETag"):
            extra = {"If-None-Match": a[1]["ETag"]}
            ok &= compare(path + " [If-None-Match]", fetch(args.django, path, args.auth, extra=extra),
                          fetch(args.rust, path, args.auth, extra=extra))
        if a[1].get("Last-Modified"):
            extra = {"If-Modified-Since": a[1]["Last-Modified"]}
            ok &= compare(path + " [If-Modified-Since]", fetch(args.django, path, args.auth, extra=extra),
                          fetch(args.rust, path, args.auth, extra=extra))
    # unauthenticated
    for d in args.doc[:1]:
        path = f"/api/documents/{d}/download/"
        ok &= compare(path + " [anon]", fetch(args.django, path), fetch(args.rust, path))
    for slug in args.share + ["123notaslug"]:
        for path in (f"/share/{slug}", f"/share/{slug}/"):
            ok &= compare(path, fetch(args.django, path), fetch(args.rust, path))
    for path in ("/logo/", "/logo/x.png", "/favicon.ico"):
        ok &= compare(path, fetch(args.django, path), fetch(args.rust, path))
    for spec in args.bulk:
        ids = [int(x) for x in spec.split(",") if x]
        for content in ("archive", "originals", "both"):
            for ff in (False, True):
                body = json.dumps({"documents": ids, "content": content, "follow_formatting": ff}).encode()
                h = {"Content-Type": "application/json"}
                name = f"bulk {ids} {content} ff={ff}"
                ok &= compare(name,
                              fetch(args.django, "/api/documents/bulk_download/", args.auth, "POST", body, h),
                              fetch(args.rust, "/api/documents/bulk_download/", args.auth, "POST", body, h),
                              zip_body=True)
    for bad in (b"{}", b'{"documents": [1, "x"]}', b'{"documents": [1], "content": "foo"}', b"[]",
                b'{"documents": [1], "excluded_documents": [2]}'):
        h = {"Content-Type": "application/json"}
        ok &= compare(f"bulk invalid {bad!r}",
                      fetch(args.django, "/api/documents/bulk_download/", args.auth, "POST", bad, h),
                      fetch(args.rust, "/api/documents/bulk_download/", args.auth, "POST", bad, h))
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
