"""W14 end-to-end test: task dispatch of the handler crates through pl_core::tasks::send_task
into the PgQueue, executed by the embedded Rust worker (no Django, no Redis).

Runs against PaperRust on 127.0.0.1:8160 (`run_server.sh`, DB pl_w14_test, PAPERRUST_QUEUE=postgres):

    bash reset_e2e.sh && bash run_server.sh &   # then
    reference/venv/Scripts/python.exe -I e2e_test.py [E2E_DIR]

Checks:
1. POST /api/tasks/run/ {"task_type": "train_classifier"} (pl-system) -> task in
   paperrust_task_queue, PaperlessTask (trigger_source manual) SUCCESS by the Rust worker.
2. POST /api/share_link_bundles/ (pl-fields-views) -> build_share_link_bundle -> status ready,
   zip file on disk; POST .../rebuild/ -> second task -> ready again.
3. pg_terminate_backend on the server's own connections -> next request 200 (pl-authz retry).
"""

import json
import os
import sys
import time
import urllib.request
import zipfile

import psycopg

BASE = "http://127.0.0.1:8160"
DSN = "host=127.0.0.1 port=54329 dbname=pl_w14_test user=paperless password=paperless"
ADMIN = "Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"
E2E = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.environ.get("TMPDIR", "/tmp"), "paperrust-w14-e2e")
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail and not ok else ""))


def req(method, path, body=None, token=ADMIN):
    data = None if body is None else json.dumps(body).encode()
    r = urllib.request.Request(BASE + path, data=data, method=method)
    r.add_header("Authorization", token)
    r.add_header("Accept", "application/json; version=9")
    if data is not None:
        r.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(r, timeout=60) as resp:
            raw = resp.read()
            return resp.status, (json.loads(raw) if raw else None)
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return e.code, json.loads(raw)
        except Exception:
            return e.code, raw[:300]


def wait_task(conn, task_id, timeout=120):
    end = time.time() + timeout
    row = None
    while time.time() < end:
        row = conn.execute(
            "SELECT status, trigger_source, task_type, result_data FROM documents_paperlesstask WHERE task_id = %s",
            (task_id,),
        ).fetchone()
        if row and row[0] in ("success", "failure"):
            return row
        time.sleep(0.5)
    return row


def wait_bundle(bid, timeout=120):
    end = time.time() + timeout
    b = None
    while time.time() < end:
        st, b = req("GET", f"/api/share_link_bundles/{bid}/")
        if st == 200 and b["status"] in ("ready", "failed"):
            return b
        time.sleep(0.5)
    return b


def main():
    conn = psycopg.connect(DSN, autocommit=True)

    st, _ = req("GET", "/api/tags/")
    check("server up (GET /api/tags/ 200)", st == 200, st)

    # 1. tasks.run -> Rust worker
    st, body = req("POST", "/api/tasks/run/", {"task_type": "train_classifier"})
    check("tasks.run 200 + task_id", st == 200 and isinstance(body, dict) and "task_id" in body, f"{st} {body}")
    if st == 200:
        tid = body["task_id"]
        q = conn.execute("SELECT task_name, headers FROM paperrust_task_queue WHERE task_id = %s", (tid,)).fetchone()
        check("tasks.run: row in paperrust_task_queue", q and q[0] == "documents.tasks.train_classifier", q)
        check("tasks.run: header trigger_source=manual", q and (q[1] or {}).get("trigger_source") == "manual", q)
        row = wait_task(conn, tid)
        check("tasks.run: PaperlessTask success by Rust worker", row and row[0] == "success", row)
        check("tasks.run: PaperlessTask trigger_source manual, type train_classifier",
              row and row[1] == "manual" and row[2] == "train_classifier", row)

    # 2. share_link_bundle create + rebuild -> Rust worker
    st, b = req("POST", "/api/share_link_bundles/", {"document_ids": [3, 4], "file_version": "original"})
    check("share_link_bundles.create 201", st == 201, f"{st} {b}")
    if st == 201:
        bid, slug = b["id"], b["slug"]
        b1 = wait_bundle(bid)
        check("bundle built: status ready", b1 and b1["status"] == "ready", b1)
        path = os.path.join(E2E, "media", "documents", "share_link_bundles", f"{slug}.zip")
        ok = os.path.isfile(path)
        check("bundle zip on disk", ok, path)
        if ok:
            with zipfile.ZipFile(path) as z:
                names = z.namelist()
                check("bundle zip has 2 entries, valid CRC", len(names) == 2 and z.testzip() is None, names)
            check("bundle size_bytes == file size", b1["size_bytes"] == os.path.getsize(path), b1)
        st, rb = req("POST", f"/api/share_link_bundles/{bid}/rebuild/")
        check("share_link_bundles.rebuild 200 pending", st == 200 and rb["status"] == "pending", f"{st} {rb}")
        b2 = wait_bundle(bid)
        check("bundle rebuilt: ready, new built_at",
              b2 and b2["status"] == "ready" and b2["built_at"] and b2["built_at"] != b1["built_at"], (b1, b2))
        rows = conn.execute(
            "SELECT t.status, t.trigger_source FROM documents_paperlesstask t JOIN paperrust_task_queue q "
            "ON q.task_id::text = t.task_id WHERE q.task_name = 'documents.tasks.build_share_link_bundle' ORDER BY t.id"
        ).fetchall()
        check("2 build_share_link_bundle tasks success (manual)",
              len(rows) == 2 and all(r == ("success", "manual") for r in rows), rows)

    # 3. stale connections: kill the server's own pool connections, next request 200
    killed = conn.execute(
        "SELECT count(*) FILTER (WHERE pg_terminate_backend(pid)) FROM pg_stat_activity "
        "WHERE datname = 'pl_w14_test' AND pid <> pg_backend_pid() AND application_name = 'paperrust'"
    ).fetchone()[0]
    time.sleep(0.5)
    st, _ = req("GET", "/api/tags/")
    check(f"after pg_terminate_backend ({killed} conns) GET /api/tags/ 200", killed > 0 and st == 200, (killed, st))
    st, _ = req("GET", "/api/share_link_bundles/")
    check("second request after kill 200", st == 200, st)

    failed = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
