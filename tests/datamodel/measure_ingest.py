#!/usr/bin/env python3
"""Einlesen profilieren (docs/DATAMODEL-V2.md §6 „Stufe 6“, docs/PERF-SCALE.md §12.3).

Lädt --n Textdateien über die Upload-Route hoch (wie run.py --scenarios ingest), wartet auf die
Aufgaben und zählt im Server-Log (PAPERRUST_LOG mit sqlx::query=debug) die SQL-Anweisungen im
Zeitraum des Einlesens – ohne Sequenzer und Warteschlangen-Abfragen. Ergebnis: Dauer und Anweisungen
je Dokument, die häufigsten Anweisungen. Räumt die Dokumente danach wieder weg.

  "$PY" -I tests/datamodel/measure_ingest.py --base http://127.0.0.1:58461 --log <work>/serve-x.log \
      --dsn "host=127.0.0.1 port=54329 dbname=pl_dm6_scale user=paperless password=paperless" --out results/…/ingest.json
"""
from __future__ import annotations

import argparse
import collections
import json
import re
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "bench" / "scale"))
import run as scale  # noqa: E402

# Requests of the measurement script itself (login per request, querying task status) do not count
SKIP = ("u.password", "\\\"t\\\".\\\"task_id\\\"", "paperrust_search_dirty", "set_config(", "paperrust_sync_sequence", "paperrust_task_queue", "paperrust_label_count_fold", "pg_notify", "LISTEN", "paperrust_guard")
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def parse(log: Path, pos: int) -> collections.Counter:
    c: collections.Counter = collections.Counter()
    with log.open("r", encoding="utf-8", errors="replace") as f:
        f.seek(pos)
        for line in f:
            line = ANSI.sub("", line)
            if "sqlx::query" not in line:
                continue
            m = re.search(r'summary="([^"]*)"', line)
            if not m:
                continue
            s = m.group(1)
            st = re.search(r'db\.statement="((?:[^"\\]|\\.)*)"', line)
            full = (st.group(1).replace("\\n", " ").strip() if st and st.group(1) else s)
            if any(k in full or k in s for k in SKIP):
                continue
            key = re.sub(r"\s+", " ", re.sub(r"\$\d+(, \$\d+)*", "$n", full))[:110]
            c[key] += 1
    return c


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--dsn", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--label", default="")
    ap.add_argument("--n", type=int, default=30)
    a = ap.parse_args()
    scale.REQ_TIMEOUT, scale.DSN = 120, a.dsn
    log = Path(a.log)
    scale.scenario_ingest(a.base, a.dsn, 3)  # warm (plans, caches)
    time.sleep(2)
    pos = log.stat().st_size
    res = scale.scenario_ingest(a.base, a.dsn, a.n)
    time.sleep(1)
    # Cleanup (bulk delete + trash) is at the end of scenario_ingest and counts: reporting it
    # separately is not possible, so only what lies before the first bulk_edit access counts
    counts = parse(log, pos)
    total = sum(counts.values())
    out = {
        "label": a.label, "n": res.get("accepted"), "per_doc_ms": res.get("per_doc_ms"), "task_status": res.get("task_status"),
        "queries_total_incl_cleanup": total, "queries_per_doc_incl_cleanup": round(total / max(1, res.get("accepted") or 1), 1),
        "top": [{"n_per_doc": round(n / max(1, res.get("accepted") or 1), 2), "sql": k} for k, n in counts.most_common(45)],
    }
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(out, indent=1, ensure_ascii=False), encoding="utf-8")
    print(json.dumps({k: v for k, v in out.items() if k != "top"}))
    for t in out["top"][:45]:
        print(f'{t["n_per_doc"]:7.2f}  {t["sql"]}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
