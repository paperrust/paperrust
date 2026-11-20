"""W1 end-to-end test: upload -> task -> document -> search -> websocket, without Django.

Runs against PaperRust on 127.0.0.1:8110 (`run_server.sh`, DB pl_w1_test). Python of the
reference venv (aiohttp, psycopg, django for session signing):

    reference/venv/Scripts/python.exe -I e2e_test.py
"""

import asyncio
import json
import sys
import time
import uuid

import aiohttp
import psycopg

BASE = "http://127.0.0.1:8110"
WS = "ws://127.0.0.1:8110/ws/status/"
SECRET = "paperless-rs-reference-secret-key-local-only-0123456789"
DSN = "host=127.0.0.1 port=54329 dbname=pl_w1_test user=paperless password=paperless"
ALICE = "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail and not ok else ""))


def make_session(conn, user_id):
    """Django session (cached_db) for user_id, signed like SessionBase.encode."""
    from django.conf import settings

    if not settings.configured:
        settings.configure(SECRET_KEY=SECRET, USE_TZ=True)
    from django.core import signing
    from django.utils.crypto import salted_hmac

    pw = conn.execute("SELECT password FROM auth_user WHERE id = %s", (user_id,)).fetchone()[0]
    h = salted_hmac(
        "django.contrib.auth.models.AbstractBaseUser.get_session_auth_hash", pw, algorithm="sha256"
    ).hexdigest()
    data = {
        "_auth_user_id": str(user_id),
        "_auth_user_backend": "django.contrib.auth.backends.ModelBackend",
        "_auth_user_hash": h,
    }
    encoded = signing.dumps(
        data, salt="django.contrib.sessions.SessionStore", serializer=signing.JSONSerializer, compress=True
    )
    key = "w1e2e" + uuid.uuid4().hex[:27]
    conn.execute(
        "INSERT INTO django_session (session_key, session_data, expire_date) VALUES (%s, %s, now() + interval '1 day')",
        (key, encoded),
    )
    return key


async def ws_collect(session, cookie, sink, ready):
    async with session.ws_connect(WS, headers={"Cookie": f"sessionid={cookie}"}) as ws:
        ready.set()
        async for msg in ws:
            if msg.type == aiohttp.WSMsgType.TEXT:
                sink.append(msg.data)


async def wait_task(session, task_id, timeout=180):
    deadline = time.time() + timeout
    while time.time() < deadline:
        async with session.get(f"{BASE}/api/tasks/?task_id={task_id}", headers={"Authorization": ALICE}) as r:
            body = await r.json()
        items = body if isinstance(body, list) else body.get("results", [])
        if items and items[0]["status"] in ("success", "failure", "revoked"):
            return items[0]
        await asyncio.sleep(0.5)
    return items[0] if items else None


async def upload(session, name, data, fields):
    form = aiohttp.FormData()
    for k, v in fields:
        form.add_field(k, v)
    form.add_field("document", data, filename=name, content_type="application/octet-stream")
    async with session.post(f"{BASE}/api/documents/post_document/", data=form, headers={"Authorization": ALICE}) as r:
        return r.status, await r.text()


async def main():
    with psycopg.connect(DSN, autocommit=True) as conn:
        admin_sid = make_session(conn, 2)
        bob_sid = make_session(conn, 4)
        tag_id = conn.execute("SELECT id FROM documents_tag WHERE NOT is_inbox_tag ORDER BY id LIMIT 1").fetchone()[0]

    async with aiohttp.ClientSession() as s:
        # anonymous websocket -> 403 (like Channels/granian)
        async with s.get(WS, headers={"Connection": "Upgrade", "Upgrade": "websocket", "Sec-WebSocket-Version": "13",
                                      "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ=="}) as r:
            check("ws anonymous -> 403", r.status == 403, r.status)
        admin_msgs, bob_msgs = [], []
        ra, rb = asyncio.Event(), asyncio.Event()
        ta = asyncio.create_task(ws_collect(s, admin_sid, admin_msgs, ra))
        tb = asyncio.create_task(ws_collect(s, bob_sid, bob_msgs, rb))
        await asyncio.wait_for(asyncio.gather(ra.wait(), rb.wait()), 10)
        check("ws session -> connected", True)

        # validation through the server
        async with s.post(f"{BASE}/api/documents/post_document/", json={}, headers={"Authorization": ALICE}) as r:
            check("json body -> 415", r.status == 415 and "Unsupported media type" in await r.text(), r.status)
        st, body = await upload(s, "x.bin", b"\x00\x01\x02binary", [("tags", "99999")])
        check("validation errors -> 400", st == 400 and json.loads(body) == {
            "document": ["File type application/octet-stream not supported"],
            "tags": ['Invalid pk "99999" - object does not exist.'],
        }, body)
        async with s.post(f"{BASE}/api/documents/post_document/", data=b"", headers={"Authorization": "Token c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004"}) as r:
            check("no add_document -> 403 Insufficient permissions", r.status == 403 and await r.text() == "Insufficient permissions", r.status)

        # 1) small text upload with metadata
        word = "quokka" + uuid.uuid4().hex[:8]
        text = f"W1 end to end test document {word}\nRechnung vom 3. Februar 2025\n".encode()
        st, body = await upload(s, "w1-e2e.txt", text, [("title", "W1 E2E"), ("tags", str(tag_id)), ("from_webui", "true")])
        check("upload -> 200 task id", st == 200 and len(json.loads(body)) == 36, body)
        task_id = json.loads(body)
        task = await wait_task(s, task_id)
        check("task in /api/tasks/ -> SUCCESS", task and task["status"] == "success", task)
        check("task type/source", task and task["task_type"] == "consume_file" and task["trigger_source"] == "web_ui", task)
        doc_id = task and (task.get("related_document_ids") or [None])[0]
        check("document created", doc_id is not None, task)
        async with s.get(f"{BASE}/api/documents/{doc_id}/", headers={"Authorization": ALICE}) as r:
            d = await r.json()
        # title: the seed's "Document Added" workflow assigns its own title template (as upstream would)
        check("document fields", r.status == 200 and tag_id in d["tags"] and d["owner"] == 3 and d["original_file_name"] == "w1-e2e.txt" and word in d["content"], d)
        for _ in range(20):  # reader reload OnCommitWithDelay (like tantivy-py)
            async with s.get(f"{BASE}/api/documents/?query={word}", headers={"Authorization": ALICE}) as r:
                sr = await r.json()
            if sr.get("count"):
                break
            await asyncio.sleep(0.5)
        check("search finds document", r.status == 200 and [x["id"] for x in sr["results"]] == [doc_id], sr)

        # 2) large upload (> 2.5 MiB, streamed to disk; Django would be needed before)
        big_word = "wombat" + uuid.uuid4().hex[:8]
        big = (f"{big_word} start\n" + ("lorem ipsum dolor sit amet " * 40 + "\n") * 2800).encode()
        check("large file > 2.5 MiB", len(big) > 2.5 * 1024 * 1024, len(big))
        st, body = await upload(s, "w1-big.txt", big, [])
        check("large upload -> 200", st == 200, body)
        task2 = await wait_task(s, json.loads(body)) if st == 200 else None
        check("large upload task SUCCESS", task2 and task2["status"] == "success", task2)
        # the tantivy reader reloads on the meta.json file watcher (~0.5-1 s after the commit)
        for _ in range(20):
            async with s.get(f"{BASE}/api/documents/?query={big_word}", headers={"Authorization": ALICE}) as r:
                sr2 = await r.json()
            if sr2.get("count"):
                break
            await asyncio.sleep(0.5)
        check("large document searchable", r.status == 200 and sr2.get("count") == 1, sr2)

        await asyncio.sleep(1)
        ta.cancel()
        tb.cancel()

    ev = [json.loads(m) for m in admin_msgs]
    mine = [e for e in ev if e.get("type") == "status_update" and e["data"]["task_id"] == task_id]
    statuses = [e["data"]["status"] for e in mine]
    check("ws admin: STARTED..SUCCESS for task", statuses[:1] == ["STARTED"] and statuses[-1:] == ["SUCCESS"], statuses)
    check("ws progress 0..100", [e["data"]["current_progress"] for e in mine][::len(mine) - 1 or 1] == [0, 100] if mine else False)
    check("ws success has document_id", mine and mine[-1]["data"]["document_id"] == doc_id, mine[-1:] if mine else None)
    check("ws json.dumps format", admin_msgs and any(m.startswith('{"type": "status_update", "data": {"filename": ') for m in admin_msgs), admin_msgs[:1])
    bob_task = [m for m in bob_msgs if task_id in m]
    check("ws bob (no view right) gets nothing of alice's task", not bob_task, bob_task[:1])
    print(f"\n{sum(ok for _, ok in results)}/{len(results)} passed")
    print("admin ws messages:", len(admin_msgs), "bob:", len(bob_msgs))
    for m in bob_msgs:
        print("   bob:", m[:300])
    for m in mine:
        print("  ", m["data"]["status"], m["data"]["current_progress"], m["data"]["message"])
    return all(ok for _, ok in results)


if __name__ == "__main__":
    sys.exit(0 if asyncio.run(main()) else 1)
