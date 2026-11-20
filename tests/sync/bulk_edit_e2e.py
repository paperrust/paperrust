"""bulk_edit end to end with/without sync triggers (ROADMAP-NEXT P2: "bulk_edit 10,000 documents with
triggers at most +20 %").

Measures `POST /api/documents/bulk_edit/` (add_tag, remove_tag, set_correspondent on 10,000 documents)
against a running PaperRust test server on the OWN 100k DB of load_bench.py
(`python -I tests/sync/load_bench.py --keep`), alternating without (sync completely removed) and with
applied migration. The result is appended to the newest results/sync/load-*.md.

    python -I tests/sync/bulk_edit_e2e.py --url http://127.0.0.1:8121 [--rounds 3]
"""

from __future__ import annotations

import argparse
import json
import statistics
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import load_bench as lb  # noqa: E402

ADMIN = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"


def post(url: str, body: dict) -> float:
    req = urllib.request.Request(url + "/api/documents/bulk_edit/", data=json.dumps(body).encode(), method="POST",
                                 headers={"Authorization": f"Token {ADMIN}", "Content-Type": "application/json",
                                          "Accept": "application/json; version=9"})
    t = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            r.read()
            if r.status != 200:
                raise RuntimeError(f"HTTP {r.status}")
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"HTTP {e.code}: {e.read()[:300]!r}") from None
    return (time.perf_counter() - t) * 1000


def set_capture(on: bool) -> None:
    installed = lb.psql("SELECT to_regclass('paperrust_sync_log') IS NOT NULL").strip() == "t"
    if installed:
        lb.psql("SELECT paperrust_sync_drop_triggers()")
        lb.psql("DROP VIEW IF EXISTS paperrust_sync_live; DROP TABLE paperrust_sync_object, paperrust_sync_pending, "
                "paperrust_sync_log, paperrust_sync_meta; DROP SEQUENCE paperrust_sync_seq")
    if on:
        lb.psql(file=lb.MIGRATION, single_tx=True)
    lb.psql("VACUUM ANALYZE documents_document, documents_document_tags")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--rounds", type=int, default=3)
    a = ap.parse_args()
    docs = [int(x) for x in lb.psql("SELECT id FROM documents_document WHERE deleted_at IS NULL "
                                    "ORDER BY id LIMIT 10000").split()]
    tag = int(lb.psql("SELECT max(id) FROM documents_tag").strip())
    corr = [int(x) for x in lb.psql("SELECT id FROM documents_correspondent ORDER BY id LIMIT 2").split()]
    ops = {
        "add_tag": {"documents": docs, "method": "add_tag", "parameters": {"tag": tag}},
        "remove_tag": {"documents": docs, "method": "remove_tag", "parameters": {"tag": tag}},
        "set_correspondent": {"documents": docs, "method": "set_correspondent", "parameters": {"correspondent": corr[0]}},
        "set_correspondent (zurück)": {"documents": docs, "method": "set_correspondent",
                                       "parameters": {"correspondent": corr[1]}},
    }
    res: dict[tuple[str, bool], list[float]] = {}
    for r in range(a.rounds):
        for on in (False, True) if r % 2 == 0 else (True, False):
            set_capture(on)
            for name, body in ops.items():
                res.setdefault((name, on), []).append(post(a.url, body))
            if on:
                lb.psql("SELECT paperrust_sync_sequence(100000)")
            print(f"round {r + 1} capture={'on' if on else 'off'} done", flush=True)
    set_capture(True)
    L = ["", "## bulk_edit Ende-zu-Ende (PaperRust-API, 10 000 Dokumente)", "",
         f"- `POST /api/documents/bulk_edit/` gegen PaperRust ({a.url}, Debug-Build, Route `documents.bulk_edit` "
         f"= rust) auf `{lb.DB}`; je Fall Median aus {a.rounds} Runden, ohne/mit abwechselnd; Werkzeug "
         "`tests/sync/bulk_edit_e2e.py`", "",
         "| Methode | ohne (ms) | mit (ms) | Δ |", "|---|---:|---:|---:|"]
    for name in ops:
        b, t = statistics.median(res[(name, False)]), statistics.median(res[(name, True)])
        L.append(f"| {name} | {b:.0f} | {t:.0f} | {(t / b - 1) * 100:+.0f} % |")
    reports = sorted((lb.REPO / "results" / "sync").glob("load-*.md"))
    if reports:
        with reports[-1].open("a", encoding="utf-8") as f:
            f.write("\n".join(L) + "\n")
    sys.stdout.reconfigure(errors="replace")
    print("\n".join(L))
    return 0


if __name__ == "__main__":
    sys.exit(main())
