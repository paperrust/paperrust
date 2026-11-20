"""W10 end-to-end test: AI in the embedded worker (no Django, no Celery, no Redis, no cloud).

Upload -> consume_file -> update_document_in_llm_index (PaperRust queue) -> LLM index written
-> GET /api/documents/{id}/ai_suggestions/ answers from the local mock; automatic processing
"suggest" after consumption (paperrust.ai.process_document -> paperrust_ai_review); trigger tag
via beat (paperrust.ai.process_trigger_tag); migration 0001_paperrust_ai applied at start.

Runs against PaperRust on 127.0.0.1:8130 (`run_server.sh`, DB pl_w10_test) with the mock on
127.0.0.1:8131 (`run_mock.sh`):

    reference/venv/Scripts/python.exe -I e2e_ai_test.py <E2E_DIR>
"""

import asyncio
import json
import os
import sys
import time
import uuid

import aiohttp
import psycopg

BASE = "http://127.0.0.1:8130"
DSN = "host=127.0.0.1 port=54329 dbname=pl_w10_test user=paperless password=paperless"
ALICE = "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"
TRIGGER_TAG = "w10-ai-trigger"
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{str(detail)[:600]}]" if detail and not ok else ""))


async def wait_task(session, task_id, timeout=180):
    deadline = time.time() + timeout
    items = []
    while time.time() < deadline:
        async with session.get(f"{BASE}/api/tasks/?task_id={task_id}", headers={"Authorization": ALICE}) as r:
            body = await r.json()
        items = body if isinstance(body, list) else body.get("results", [])
        if items and items[0]["status"] in ("success", "failure", "revoked"):
            return items[0]
        await asyncio.sleep(0.5)
    return items[0] if items else None


def wait_queue(conn, task_name, doc_id=None, timeout=120):
    """Finished row of the PaperRust queue (state done/failed) for task_name (+ document_id)."""
    deadline = time.time() + timeout
    row = None
    while time.time() < deadline:
        q = "SELECT state, kwargs, result, error FROM paperrust_task_queue WHERE task_name = %s"
        args = [task_name]
        if doc_id is not None:
            q += " AND (kwargs->>'document_id')::int = %s"
            args.append(doc_id)
        row = conn.execute(q + " ORDER BY id DESC LIMIT 1", args).fetchone()
        if row and row[0] in ("done", "failed", "revoked"):
            return row
        time.sleep(0.5)
    return row


async def main(e2e_dir):
    mock_log = os.path.join(e2e_dir, "mock.log")
    with psycopg.connect(DSN, autocommit=True) as conn:
        tables = {r[0] for r in conn.execute(
            "SELECT tablename FROM pg_tables WHERE tablename LIKE 'paperrust_%'").fetchall()}
        check("migration 0001_paperrust_ai applied at start",
              {"paperrust_ai_review", "paperrust_ai_audit", "paperrust_taxonomy_alias"} <= tables, tables)
        beat = conn.execute("SELECT name FROM paperrust_beat_state ORDER BY name").fetchall()
        names = [b[0] for b in beat]
        check("beat: 'Rebuild LLM index' scheduled (task now ported)", "Rebuild LLM index" in names, names)
        check("beat: PaperRust AI trigger tag entry", "PaperRust: process AI trigger tag" in names, names)
        trigger_tag = conn.execute("SELECT id FROM documents_tag WHERE name = %s", (TRIGGER_TAG,)).fetchone()[0]

        async with aiohttp.ClientSession() as s:
            word = "numbat" + uuid.uuid4().hex[:8]
            text = f"W10 AI end to end document {word}\nInvoice from ACME Corporation, 31 May 2024\nTotal 42.00 CHF\n".encode()
            form = aiohttp.FormData()
            form.add_field("title", "W10 AI E2E")
            form.add_field("document", text, filename="w10-ai.txt", content_type="text/plain")
            async with s.post(f"{BASE}/api/documents/post_document/", data=form, headers={"Authorization": ALICE}) as r:
                st, body = r.status, await r.text()
            check("upload -> 200 task id", st == 200, body)
            task = await wait_task(s, json.loads(body))
            check("consume_file SUCCESS", task and task["status"] == "success", task)
            doc_id = task and (task.get("related_document_ids") or [None])[0]
            check("document created", doc_id is not None, task)

            # document_consumption_finished -> add_or_update_document_in_llm_index
            row = wait_queue(conn, "documents.tasks.update_document_in_llm_index", doc_id)
            check("update_document_in_llm_index queued + done", row and row[0] == "done", row)
            idx = os.path.join(e2e_dir, "data", "llm_index", "llmindex.db")
            check("LLM index file written", os.path.isfile(idx), idx)
            log = open(mock_log, encoding="utf-8").read() if os.path.isfile(mock_log) else ""
            embeds = [json.loads(l) for l in log.splitlines() if l.strip() and json.loads(l).get("path") == "/api/embed"]
            check("embedding request for the new document (mock)", any(word in json.dumps(e) for e in embeds), len(embeds))

            # ai_suggestions answers from the mock (RAG over the index the worker just wrote)
            async with s.get(f"{BASE}/api/documents/{doc_id}/ai_suggestions/", headers={"Authorization": ALICE}) as r:
                st, sug = r.status, await r.text()
            ok = st == 200
            sj = json.loads(sug) if ok else {}
            check("ai_suggestions -> 200", ok, f"{st} {sug}")
            # mock: title "Mock <hash>" or, with an output-language hint in the prompt, "Übersetzt <hash>"
            check("ai_suggestions result from mock (title, dates)",
                  str(sj.get("title", "")).startswith(("Mock ", "Übersetzt ")) and sj.get("dates") == ["2024-05-31"], sj)
            log = open(mock_log, encoding="utf-8").read() if os.path.isfile(mock_log) else ""
            chats = [l for l in log.splitlines() if '"/api/chat"' in l and word in l]
            check("LLM chat request contains the document", bool(chats), len(chats))

            # PaperRust automatic processing after consumption (PAPERRUST_AI_AUTO_MODE=suggest)
            row = wait_queue(conn, "paperrust.ai.process_document", doc_id)
            check("paperrust.ai.process_document done", row and row[0] == "done", row)
            rev = conn.execute("SELECT trigger, mode, status FROM paperrust_ai_review WHERE document_id = %s", (doc_id,)).fetchall()
            check("review entry (consumed, suggest, pending)", ("consumed", "suggest", "pending") in rev, rev)
            async with s.get(f"{BASE}/api/x-ai/review/?document={doc_id}", headers={"Authorization": ALICE}) as r:
                rl = await r.json()
            check("GET /api/x-ai/review/ lists it", r.status == 200 and rl.get("count", 0) >= 1, rl)

            # trigger tag via beat (cron every minute)
            audit_q = "SELECT count(*) FROM auditlog_logentry WHERE object_pk = %s AND changes::text LIKE %s"
            audit_before = conn.execute(audit_q, (str(doc_id), "%" + TRIGGER_TAG + "%")).fetchone()[0]
            conn.execute("INSERT INTO documents_document_tags (document_id, tag_id) VALUES (%s, %s)", (doc_id, trigger_tag))
            deadline = time.time() + 150
            tagged = True
            while time.time() < deadline:
                tagged = conn.execute("SELECT EXISTS(SELECT 1 FROM documents_document_tags WHERE document_id = %s AND tag_id = %s)",
                                      (doc_id, trigger_tag)).fetchone()[0]
                if not tagged:
                    break
                time.sleep(2)
            check("beat ran process_trigger_tag: tag removed", not tagged)
            audit_after = conn.execute(audit_q, (str(doc_id), "%" + TRIGGER_TAG + "%")).fetchone()[0]
            check("tag removal through docops writes an auditlog entry", audit_after == audit_before + 1, (audit_before, audit_after))
            row = wait_queue(conn, "paperrust.ai.process_trigger_tag", None, 30)
            check("paperrust.ai.process_trigger_tag done", row and row[0] == "done", row)
            rev = conn.execute("SELECT trigger FROM paperrust_ai_review WHERE document_id = %s", (doc_id,)).fetchall()
            check("review entry with trigger 'tag'", ("tag",) in rev, rev)

            # llmindex_index (upstream task name, also what /api/tasks/run/ llmindex_update enqueues)
            tid = str(uuid.uuid4())
            conn.execute(
                "INSERT INTO paperrust_task_queue (task_id, task_name, kwargs, headers) VALUES (%s, %s, %s, %s)",
                (tid, "documents.tasks.llmindex_index", json.dumps({"rebuild": False}), json.dumps({"trigger_source": "manual"})),
            )
            row = wait_queue(conn, "documents.tasks.llmindex_index", None, 300)
            check("llmindex_index done", row and row[0] == "done", row)
            check("llmindex_index result text", row and row[2] in ("LLM index updated successfully.", "No changes detected in LLM index."), row)

    print(f"\n{sum(ok for _, ok in results)}/{len(results)} passed")
    return all(ok for _, ok in results)


if __name__ == "__main__":
    sys.exit(0 if asyncio.run(main(sys.argv[1])) else 1)
