#!/usr/bin/env python3
"""SQL-Messung Stufe 5 (Labels + Sync, docs/DATAMODEL-V2.md §6) auf einer KOPIE der Scale-DB.

Misst über die Namen, die der Code liest (paperrust_sync_object: vor Stufe 5 Tabelle, danach
Lesesicht), bestes von --runs Läufen (EXPLAIN ANALYZE, warm), dazu Schreibproben mit Erfassung
(Sync-Trigger an) und die Laufzeit des Sequenzers danach. Die Schreibproben werden committet und
anschließend zurückgenommen (gleiche Datenmenge vor/nach).

  "$PY" -I tests/datamodel/measure_sync.py --dsn "host=127.0.0.1 port=54329 dbname=pl_dm5_scale user=paperless password=paperless" \
      --label vorher --out results/datamodel/dm5-vorher/sql.json
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import psycopg

READS = {
    # sync/ids (admin: all living root documents)
    "sync_ids_documents": "SELECT o.uuid, o.rev FROM paperrust_sync_object o WHERE o.kind = 1 AND o.local_id IN "
                          "(SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL) ORDER BY o.local_id",
    "ident_fetch_25": "SELECT local_id::int8, uuid, rev FROM paperrust_sync_object WHERE kind = 1 AND local_id = ANY("
                      "ARRAY(SELECT id::int8 FROM documents_document ORDER BY id DESC LIMIT 25))",
    "ident_resolve_25": "SELECT uuid, local_id::int8, rev FROM paperrust_sync_object WHERE kind = 1 AND deleted_at IS NULL AND uuid = ANY("
                        "ARRAY(SELECT uuid FROM paperrust_sync_object WHERE kind = 1 AND local_id BETWEEN 500000 AND 500024))",
    "ident_tags_all": "SELECT local_id::int8, uuid, rev FROM paperrust_sync_object WHERE kind = 2",
    "tags_list_view": "SELECT * FROM documents_tag ORDER BY name, id LIMIT 25",
    "doc_by_id": "SELECT * FROM documents_document WHERE id = 500000",
}
SIZES = """
SELECT c.relname, pg_relation_size(c.oid), pg_indexes_size(c.oid), pg_total_relation_size(c.oid)
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind = 'r'
  AND c.relname IN ('paperrust_sync_object', 'paperrust_sync_ident', 'paperrust_sync_tombstone', 'paperrust_document',
                    'paperrust_sync_log', 'documents_tag', 'paperrust_tag')
ORDER BY 1
"""


def exec_ms(cur, sql: str) -> float:
    cur.execute("EXPLAIN (ANALYZE, FORMAT JSON) " + sql)
    return float(cur.fetchone()[0][0]["Execution Time"])


def timed(conn, sql: str, params=None) -> float:
    t = time.perf_counter()
    with conn.cursor() as cur:
        cur.execute(sql, params)
    conn.commit()
    return (time.perf_counter() - t) * 1000


def drain(conn) -> tuple[float, int]:
    t = time.perf_counter()
    n = 0
    while True:
        with conn.cursor() as cur:
            cur.execute("SELECT paperrust_sync_sequence(5000)")
            k = cur.fetchone()[0]
        conn.commit()
        if k <= 0:
            break
        n += k
    return (time.perf_counter() - t) * 1000, n


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dsn", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--runs", type=int, default=3)
    a = ap.parse_args()
    out: dict = {"label": a.label, "reads_ms": {}, "writes_ms": {}, "sizes": {}}
    with psycopg.connect(a.dsn) as conn:
        drain(conn)
        with conn.cursor() as cur:
            for name, sql in READS.items():
                out["reads_ms"][name] = round(min(exec_ms(cur, sql) for _ in range(a.runs)), 2)
                print(name, out["reads_ms"][name], flush=True)
            cur.execute(SIZES)
            for rel, heap, idx, total in cur.fetchall():
                out["sizes"][rel] = {"heap_mb": round(heap / 2**20, 1), "index_mb": round(idx / 2**20, 1), "total_mb": round(total / 2**20, 1)}
            cur.execute("SELECT min(id) FROM documents_tag WHERE name NOT LIKE 'bench%'")
            tag = cur.fetchone()[0]
        conn.commit()
        w = out["writes_ms"]
        ids = "SELECT id FROM documents_document WHERE id BETWEEN 200001 AND 210000 AND root_document_id IS NULL"
        # Tag on 10,000 documents (assignment + capture), sequencer, back
        with conn.cursor() as cur:
            cur.execute(f"CREATE TEMP TABLE m_pre AS SELECT document_id FROM documents_document_tags WHERE tag_id = %s AND document_id IN ({ids})", (tag,))
            cur.execute("SELECT count(*) FROM m_pre")
            before = cur.fetchone()[0]
        conn.commit()
        w["tag_add_10k"] = round(timed(conn, f"INSERT INTO documents_document_tags (document_id, tag_id) SELECT id, %s FROM ({ids}) x ON CONFLICT DO NOTHING", (tag,)), 1)
        w["sequencer_after_tag_add_ms"], w["sequencer_after_tag_add_n"] = drain(conn)
        w["tag_remove_10k"] = round(timed(conn, f"DELETE FROM documents_document_tags WHERE tag_id = %s AND document_id IN ({ids}) AND document_id NOT IN (SELECT document_id FROM m_pre)", (tag,)), 1)
        w["sequencer_after_tag_remove_ms"], _ = drain(conn)
        if before:
            print("Hinweis: Tag war schon an", before, "Dokumenten (bleiben)")
        # Set correspondent on 10,000 documents and back (document row)
        with conn.cursor() as cur:
            cur.execute("CREATE TEMP TABLE m_corr AS SELECT id, correspondent_id FROM paperrust_document WHERE id BETWEEN 300001 AND 310000")
        conn.commit()
        w["corr_set_10k"] = round(timed(conn, "UPDATE documents_document SET correspondent_id = NULL WHERE id BETWEEN 300001 AND 310000"), 1)
        w["sequencer_after_corr_ms"], _ = drain(conn)
        w["corr_restore_10k"] = round(timed(conn, "UPDATE documents_document d SET correspondent_id = m.correspondent_id FROM m_corr m WHERE m.id = d.id"), 1)
        drain(conn)
        # one document: note + title
        w["note_insert_one"] = round(timed(conn, "INSERT INTO documents_note (note, created, document_id, user_id) VALUES ('m', now(), 400000, NULL)"), 2)
        w["note_delete_one"] = round(timed(conn, "DELETE FROM documents_note WHERE document_id = 400000 AND note = 'm'"), 2)
        w["title_update_one"] = round(timed(conn, "UPDATE documents_document SET title = title WHERE id = 400001"), 2)
        drain(conn)
        for k in list(w):
            w[k] = round(w[k], 1) if isinstance(w[k], float) else w[k]
        print(json.dumps(w), flush=True)
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(out, indent=1, ensure_ascii=False), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
