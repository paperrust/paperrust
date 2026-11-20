"""SYNC.md §9 "Django writers": the unchanged paperless-ngx (reference Django) writes via
its API, the sync feed must contain every change – also those where Django does not change `modified`
(bulk_edit add_tag/remove_tag, notes, permissions).

Prerequisite: an OWN Django instance on a DB copy with applied migration, e.g.
the W2 copy with switch:

    W2_NAME=w2sync W2_DJ_PORT=8110 W2_RS_PORT=8111 PARITY_SYNC_CAPTURE=on bash tests/parity/w2-reset.sh
    W2_NAME=w2sync W2_DJ_PORT=8110 W2_RS_PORT=8111 PARITY_SYNC_CAPTURE=on bash tests/parity/w2-start.sh dj
    python -I tests/sync/django_capture.py --url http://127.0.0.1:8110 --db pl_w2sync_ref

The script calls the sequencer itself (`SELECT paperrust_sync_sequence()`), a running
PaperRust worker does not interfere (advisory lock). Test users/tokens from reference/README.md.
Exit 0 = all checks green.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PSQL = REPO / "reference" / "pg" / "pgsql" / "bin" / "psql.exe"
ADMIN = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"


def sql(db: str, q: str) -> list[list[str]]:
    r = subprocess.run([str(PSQL), "-X", "-q", "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1",
                        "-p", "54329", "-U", "paperless", "-d", db, "-c", q],
                       capture_output=True, env=dict(os.environ, PGPASSWORD="paperless"), timeout=120)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.decode("utf-8", "replace"))
    return [line.split("\t") for line in r.stdout.decode("utf-8").splitlines() if line]


def api(base: str, method: str, path: str, body: dict | None = None) -> tuple[int, object]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method, headers={
        "Authorization": f"Token {ADMIN}", "Accept": "application/json; version=9",
        "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            raw = r.read()
            return r.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")[:300]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--db", required=True)
    a = ap.parse_args()
    assert a.db not in ("paperless", "paperless_snapshot"), "never against the shared reference"
    if not sql(a.db, "SELECT to_regclass('paperrust_sync_log') IS NOT NULL")[0][0] == "t":
        print("sync capture not installed in", a.db)
        return 2

    def drain() -> None:
        for _ in range(100):
            sql(a.db, "SELECT paperrust_sync_sequence()")
            if sql(a.db, "SELECT count(*) FROM paperrust_sync_pending")[0][0] == "0":
                return
            time.sleep(0.2)
        raise RuntimeError("pending not drained")

    drain()
    start = int(sql(a.db, "SELECT coalesce(max(seq), 0) FROM paperrust_sync_log")[0][0])
    docs = [int(r[0]) for r in sql(a.db, "SELECT id FROM documents_document WHERE deleted_at IS NULL "
                                          "AND root_document_id IS NULL ORDER BY id LIMIT 4")]
    tag = int(sql(a.db, "SELECT id FROM documents_tag WHERE NOT is_inbox_tag ORDER BY id LIMIT 1")[0][0])
    d_tag, d_note, d_del, d_owner = docs
    modified_before = {r[0]: r[1] for r in sql(a.db, f"SELECT id, modified FROM documents_document WHERE id IN ({d_tag})")}
    checks: list[tuple[str, bool, str]] = []

    # 1. bulk_edit add_tag (bulk_create, without modified) and remove_tag
    st, body = api(a.url, "POST", "/api/documents/bulk_edit/",
                   {"documents": [d_tag], "method": "add_tag", "parameters": {"tag": tag}})
    checks.append(("bulk_edit add_tag HTTP", st == 200, f"{st} {body}"))
    # 2. Note
    st, body = api(a.url, "POST", f"/api/documents/{d_note}/notes/", {"note": "sync capture test"})
    checks.append(("add note HTTP", st == 200, f"{st}"))
    # 3. Delete (trash)
    st, body = api(a.url, "DELETE", f"/api/documents/{d_del}/")
    checks.append(("delete document HTTP", st == 204, f"{st} {body}"))
    # 4. Permissions: bulk_edit set_permissions (owner + guardian)
    st, body = api(a.url, "POST", "/api/documents/bulk_edit/",
                   {"documents": [d_owner], "method": "set_permissions",
                    "parameters": {"owner": 3, "set_permissions": {"view": {"users": [4], "groups": []},
                                                                   "change": {"users": [], "groups": []}}}})
    checks.append(("bulk_edit set_permissions HTTP", st == 200, f"{st} {body}"))
    # 5. Rename tag (object kind tag)
    st, body = api(a.url, "PATCH", f"/api/tags/{tag}/", {"name": "sync-capture-renamed"})
    checks.append(("rename tag HTTP", st == 200, f"{st}"))

    modified_after = {r[0]: r[1] for r in sql(a.db, f"SELECT id, modified FROM documents_document WHERE id IN ({d_tag})")}
    drain()
    log = {(int(r[0]), int(r[1])): (int(r[2]), r[3], r[4]) for r in
           sql(a.db, f"SELECT kind, local_id, seq, op, coalesce(perm_seq::text, '') FROM paperrust_sync_log WHERE seq > {start}")}

    def entry(kind: int, i: int):
        return log.get((kind, i))

    checks.append(("add_tag did not touch modified (precondition)", modified_before == modified_after,
                   f"{modified_before} -> {modified_after}"))
    checks.append(("feed: add_tag document upsert", bool(entry(1, d_tag)) and entry(1, d_tag)[1] == "u", str(entry(1, d_tag))))
    checks.append(("feed: noted document upsert", bool(entry(1, d_note)) and entry(1, d_note)[1] == "u", str(entry(1, d_note))))
    checks.append(("feed: trashed document delete", bool(entry(1, d_del)) and entry(1, d_del)[1] == "d", str(entry(1, d_del))))
    e = entry(1, d_owner)
    checks.append(("feed: permission change flagged", bool(e) and e[2] != "" and int(e[2]) > start, str(e)))
    checks.append(("feed: renamed tag upsert", bool(entry(2, tag)) and entry(2, tag)[1] == "u", str(entry(2, tag))))

    ok = True
    for name, good, detail in checks:
        ok &= good
        print(f"{'OK  ' if good else 'FAIL'} {name}" + ("" if good else f"  ({detail})"))
    print(f"{sum(c[1] for c in checks)}/{len(checks)} checks passed; feed entries since seq {start}: {len(log)}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
