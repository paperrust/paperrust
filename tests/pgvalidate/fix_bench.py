"""Phase 6 – fixes: timing and response comparison per measure.

One run = all selected requests once per user against ONE server; the round control
(interleaved: Rust before → Rust after → Django, 3 rounds) is done by `fix_bench.sh`.

  python -I tests/pgvalidate/fix_bench.py --label rs-before --round 1 --base http://127.0.0.1:8090 \
      --out results/pgvalidate/fix/runs.jsonl --bodies results/pgvalidate/fix/bodies

Per request one JSON line: label, round, user, name, status, ms, sha256 of the normalized
response body (keys sorted); the body itself is stored under --bodies/<label>/<user>__<name>.json
(last round overwrites), so that before/after/Django can be compared in content.
"""
import argparse
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from capture import call  # noqa: E402
from pgv_requests import REQUESTS, USERS, DL, SEL  # noqa: E402

EXTRA = [
    ("correspondents.manage.lc", "GET",
     "/api/correspondents/?page=1&page_size=25&ordering=name&full_perms=true&last_correspondence=true", None),
    ("doclist.document_type", "GET", DL.format(page=1, ord="document_type__name"), None),
    ("doclist.-storage_path", "GET", DL.format(page=1, ord="-storage_path__name"), None),
    ("doclist.f.has_duplicates+sel", "GET", DL.format(page=1, ord="-created") + "&has_duplicates=true" + SEL, None),
    # O2 (section 9): further substring searches in the content
    ("doclist.f.title_content+sel", "GET", DL.format(page=1, ord="-created") + "&title_content=rechnung" + SEL, None),
    ("doclist.f.title_content.sparse", "GET", DL.format(page=1, ord="-created") + "&title_content=strasse", None),
    ("doclist.f.content_icontains", "GET", DL.format(page=1, ord="-created") + "&content__icontains=quarterly", None),
    ("doclist.f.search", "GET", DL.format(page=1, ord="-created") + "&search=rechnung", None),
    ("doclist.f.title_content.p2", "GET", DL.format(page=2, ord="-created") + "&title_content=rechnung", None),
    ("doclist.f.title_content.umlaut", "GET", DL.format(page=1, ord="-created") + "&title_content=geb%C3%BChr", None),
]

# Requests affected by the fixes (+ controls doclist.default, selection_data)
DEFAULT = [
    "tags.listAll", "correspondents.listAll", "document_types.listAll", "storage_paths.listAll",
    "tags.manage", "tags.manage.filter", "correspondents.manage", "correspondents.manage.lc",
    "document_types.manage",
    "doclist.default", "doclist.default+sel", "doclist.f.tag_all+sel", "selection_data",
    "doclist.f.has_duplicates", "doclist.correspondent", "doclist.document_type", "doclist.-storage_path",
    "doclist.-num_notes", "statistics",
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--label", required=True)
    ap.add_argument("--round", type=int, required=True)
    ap.add_argument("--base", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--bodies", required=True)
    ap.add_argument("--users", default="admin,alice,bob")
    ap.add_argument("--only", default=",".join(DEFAULT))
    ap.add_argument("--timeout", type=float, default=900)
    a = ap.parse_args()
    reqs = {r[0]: r for r in REQUESTS + EXTRA}
    names = [n for n in a.only.split(",") if n]
    bdir = os.path.join(a.bodies, a.label)
    os.makedirs(bdir, exist_ok=True)
    for user in a.users.split(","):
        tok = USERS[user]
        _, js, _, _ = call(a.base, "GET", "/api/documents/?page=1&page_size=50&ordering=-created&fields=id", tok)
        ids = [d["id"] for d in (js or {}).get("results", [])]
        doc = ids[0] if ids else 1
        for name in names:
            _, method, path, body = reqs[name]
            p = path.replace("{DOC}", str(doc))
            b = {"documents": ids} if body == "SELECTION" else body
            status, js, ms, _ = call(a.base, method, p, tok, b, a.timeout)
            norm = json.dumps(js, sort_keys=True, ensure_ascii=False)
            sha = hashlib.sha256(norm.encode()).hexdigest()[:16]
            with open(os.path.join(bdir, f"{user}__{name}.json"), "w", encoding="utf-8") as f:
                json.dump(js, f, ensure_ascii=False, indent=1)
            rec = {"label": a.label, "round": a.round, "user": user, "name": name, "status": status,
                   "ms": round(ms, 1), "sha": sha, "t": time.strftime("%H:%M:%S")}
            with open(a.out, "a", encoding="utf-8") as f:
                f.write(json.dumps(rec) + "\n")
            print(f"{a.label} r{a.round} {user:5} {name:28} {status} {ms:10.1f} ms {sha}", flush=True)


if __name__ == "__main__":
    main()
