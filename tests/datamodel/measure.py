#!/usr/bin/env python3
"""SQL-Messung für die Umbaustufen des Datenmodells (docs/DATAMODEL-V2.md §6), je Stufe vor/nach.

Misst auf einer KOPIE der Scale-Datenbank (nie pl_scale_1m selbst) die Abfragen aus
DATAMODEL-V2 §0/§1 über die Django-Namen (= was der heutige Code liest; nach dem Umbau über die
Kompatibilitäts-Sichten). Je Abfrage bestes von --runs Läufen (warm), Ausführungszeit laut
EXPLAIN ANALYZE; Schreibproben laufen in einer zurückgerollten Transaktion.

Aufruf (Referenz-venv, hat psycopg):
  "$PY" -I tests/datamodel/measure.py --dsn "host=127.0.0.1 port=54329 dbname=pl_dm_scale user=paperless password=paperless" \
      --label vorher --out results/datamodel/dm1-vorher.json
"""
from __future__ import annotations

import argparse
import json
import re
import time
from pathlib import Path

import psycopg

LIVE = "root_document_id IS NULL AND deleted_at IS NULL"
READS = {
    "count_live": f"SELECT count(*) FROM documents_document WHERE {LIVE}",
    "page_created": f"SELECT id FROM documents_document WHERE {LIVE} ORDER BY created DESC, id DESC LIMIT 25",
    "page_created_deep": f"SELECT id FROM documents_document WHERE {LIVE} ORDER BY created DESC, id DESC LIMIT 25 OFFSET 500000",
    "page_title": f"SELECT id FROM documents_document WHERE {LIVE} ORDER BY title, id LIMIT 25",
    "page_added": f"SELECT id FROM documents_document WHERE {LIVE} ORDER BY added DESC, id DESC LIMIT 25",
    "page_rows_25": f"SELECT * FROM documents_document WHERE {LIVE} ORDER BY created DESC, id DESC LIMIT 25",
    "sum_content_length": f"SELECT count(*), sum(content_length) FROM documents_document WHERE {LIVE}",
    "count_mime_noindex": f"SELECT count(*) FROM documents_document WHERE mime_type = 'image/png' AND {LIVE}",
    "count_correspondent": f"SELECT count(*) FROM documents_document WHERE correspondent_id = (SELECT min(id) FROM documents_correspondent) AND {LIVE}",
    "checksum_lookup": "SELECT id FROM documents_document WHERE checksum = (SELECT checksum FROM documents_document WHERE id = 500000)",
    "content_icontains": f"SELECT count(*) FROM documents_document WHERE upper(content) LIKE upper('%quarterly%') AND {LIVE}",
}
WRITES = {
    # Bulk change via today's write path (view after stage 1): 10,000 documents
    "bulk_set_correspondent_10k": "UPDATE documents_document SET correspondent_id = NULL WHERE id BETWEEN 100001 AND 110000",
    "update_one_content": "UPDATE documents_document SET content = content || ' ' WHERE id = 123456",
    "insert_one": "INSERT INTO documents_document (title, content, mime_type, checksum, created, modified, added) "
                  "VALUES ('m', 'messung', 'application/pdf', 'x', '2026-01-01', now(), now())",
}
SIZES = """
SELECT c.relname, pg_relation_size(c.oid), pg_indexes_size(c.oid), pg_total_relation_size(c.oid)
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind = 'r'
  AND c.relname IN ('documents_document', 'paperrust_document', 'paperrust_document_content', 'paperrust_document_file')
ORDER BY 1
"""


def exec_ms(cur, sql: str) -> float:
    cur.execute("EXPLAIN (ANALYZE, FORMAT JSON) " + sql)
    plan = cur.fetchone()[0]
    return float(plan[0]["Execution Time"])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--dsn", required=True)
    ap.add_argument("--label", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    assert "pl_scale_1m" not in a.dsn, "nur auf einer Kopie messen"
    res: dict = {"label": a.label, "at": time.strftime("%Y-%m-%d %H:%M:%S"), "reads": {}, "writes": {}, "sizes": {}}
    with psycopg.connect(a.dsn) as conn:
        conn.autocommit = True
        with conn.cursor() as cur:
            cur.execute("SET max_parallel_workers_per_gather = 2")
            for name, sql in READS.items():
                if a.only and not re.search(a.only, name):
                    continue
                best = min(exec_ms(cur, sql) for _ in range(a.runs))
                res["reads"][name] = round(best, 1)
                print(f"{name:28} {best:10.1f} ms", flush=True)
            for name, sql in WRITES.items():
                if a.only and not re.search(a.only, name):
                    continue
                times = []
                for _ in range(a.runs):
                    cur.execute("BEGIN")
                    t = time.perf_counter()
                    cur.execute(sql)
                    times.append((time.perf_counter() - t) * 1000)
                    cur.execute("ROLLBACK")
                res["writes"][name] = round(min(times), 1)
                print(f"{name:28} {min(times):10.1f} ms (rolled back)", flush=True)
            cur.execute(SIZES)
            for rel, heap, idx, total in cur.fetchall():
                res["sizes"][rel] = {"heap_mb": round(heap / 2**20), "indexes_mb": round(idx / 2**20), "total_mb": round(total / 2**20)}
                print(f"{rel:28} heap {heap / 2**20:8.0f} MB  indexes {idx / 2**20:8.0f} MB  total {total / 2**20:8.0f} MB")
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(res, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
