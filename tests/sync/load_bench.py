"""Load measurement of the sync change capture (docs/SYNC.md §9 "Load", ROADMAP-NEXT P2).

Creates an OWN database pl_sync_bench (copy of paperless_snapshot, scaled up with
tests/pgvalidate/scale_100k.sql to ~100,000 documents), measures bulk changes without and with
triggers, the initial population (migration) and the sequencer, writes results/sync/load-<date>.md
and deletes the database again (--keep keeps it).

    python -I tests/sync/load_bench.py [--repeat 5] [--keep]

stdlib + psql of the reference environment only (reference/pg). Never against `paperless`.
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import re
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PSQL = REPO / "reference" / "pg" / "pgsql" / "bin" / "psql.exe"
DB = "pl_sync_bench"
assert DB not in ("paperless", "paperless_snapshot")
MIGRATION = REPO / "crates" / "pl-sync" / "migrations" / "0001_paperrust_sync.sql"
SCALE = REPO / "tests" / "pgvalidate" / "scale_100k.sql"


def psql(sql: str | None = None, *, db: str = DB, user: str = "paperless", file: Path | None = None,
         single_tx: bool = False) -> str:
    args = [str(PSQL), "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1", "-p", "54329",
            "-U", user, "-d", db]
    if single_tx:
        args.append("-1")
    if file is not None:
        args += ["-f", str(file)]
    if sql is not None:
        args += ["-c", sql]
    pw = "postgres-local-only" if user == "postgres" else "paperless"
    r = subprocess.run(args, capture_output=True, env=dict(os.environ, PGPASSWORD=pw), timeout=3600)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.decode("utf-8", "replace")[-800:])
    return r.stdout.decode("utf-8", "replace")


def timed(body: str, *, commit: bool = False) -> float:
    """Server-side runtime (ms) of `body` in a transaction (default: ROLLBACK)."""
    end = "COMMIT" if commit else "ROLLBACK"
    out = psql(f"""
        BEGIN;
        CREATE TEMP TABLE _t ON COMMIT DROP AS SELECT clock_timestamp() AS t0;
        {body};
        SELECT round(extract(epoch FROM clock_timestamp() - (SELECT t0 FROM _t)) * 1000, 2);
        {end};""")
    return float(out.strip().splitlines()[-1])


OPS = {
    # bulk_edit.add_tag: DocumentTagRelationship.objects.bulk_create(...) – one statement
    "add_tag 10 000 Dok. (bulk_create M2M)":
        "INSERT INTO documents_document_tags (document_id, tag_id) "
        "SELECT d.id, 1 FROM documents_document d WHERE NOT EXISTS (SELECT 1 FROM documents_document_tags x "
        "WHERE x.document_id = d.id AND x.tag_id = 1) ORDER BY d.id LIMIT 10000",
    # bulk_edit.remove_tag: DocumentTagRelationship.objects.filter(...).delete()
    "remove_tag (alle Verknüpfungen eines Tags)":
        "DELETE FROM documents_document_tags WHERE tag_id = (SELECT tag_id FROM documents_document_tags "
        "GROUP BY tag_id ORDER BY count(*) DESC LIMIT 1)",
    # bulk_edit.set_correspondent: qs.update(correspondent=…)
    "set_correspondent 10 000 Dok. (qs.update)":
        "UPDATE documents_document SET correspondent_id = (SELECT min(id) FROM documents_correspondent) "
        "WHERE id IN (SELECT id FROM documents_document ORDER BY id LIMIT 10000)",
    # bulk_edit.set_permissions: owner + guardian – here only the owner part
    "set_owner 10 000 Dok. (qs.update owner)":
        "UPDATE documents_document SET owner_id = (SELECT min(id) FROM auth_user) "
        "WHERE id IN (SELECT id FROM documents_document ORDER BY id LIMIT 10000)",
    # one statement on a fresh connection (Django without connection pool): includes the one-time
    # compilation of the trigger function per session
    "1 Einzel-UPDATE, neue Verbindung":
        "UPDATE documents_document SET modified = now() WHERE id = (SELECT min(id) FROM documents_document)",
    # Single save (Document.save per document, e.g. workflows/consumption): 1,000 statements
    "1 000 Einzel-UPDATEs (je Dokument eine Anweisung)":
        "DO $$ DECLARE i int; BEGIN FOR i IN SELECT id FROM documents_document ORDER BY id LIMIT 1000 LOOP "
        "UPDATE documents_document SET modified = now() WHERE id = i; END LOOP; END $$",
}


def explain(body: str) -> tuple[float, float]:
    """(execution time, of which time of the paperrust_sync triggers) in ms from EXPLAIN ANALYZE, ROLLBACK."""
    out = psql(f"BEGIN; EXPLAIN (ANALYZE) {body}; ROLLBACK;")
    ex = float(re.search(r"Execution Time: ([0-9.]+)", out).group(1))
    trig = sum(float(x) for x in re.findall(r"Trigger paperrust_sync\w*: time=([0-9.]+)", out))
    return ex, trig


def measure(repeat: int) -> dict[str, list[tuple[float, float]]]:
    """Per operation `repeat` measurements (total, trigger share). VACUUM beforehand so that dead rows
    of the ROLLBACK runs do not distort the following measurements."""
    res: dict[str, list[tuple[float, float]]] = {}
    for name, body in OPS.items():
        psql("VACUUM documents_document, documents_document_tags")
        if body.startswith("DO "):
            timed(body)
            res[name] = [(timed(body), 0.0) for _ in range(repeat)]
        else:
            explain(body)
            res[name] = [explain(body) for _ in range(repeat)]
    return res


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=5)
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("--reuse", action="store_true", help="vorhandene pl_sync_bench ohne Sync weiterverwenden")
    a = ap.parse_args()

    t0 = time.perf_counter()
    if a.reuse:
        # existing, scaled-up DB: remove sync completely (like `manage sync disable --purge`)
        if psql("SELECT to_regclass('paperrust_sync_log') IS NOT NULL").strip() == "t":
            psql("SELECT paperrust_sync_drop_triggers()")
            psql("DROP VIEW IF EXISTS paperrust_sync_live; DROP TABLE paperrust_sync_object, paperrust_sync_pending, "
                 "paperrust_sync_log, paperrust_sync_meta; DROP SEQUENCE paperrust_sync_seq")
        psql("VACUUM ANALYZE documents_document, documents_document_tags")
    else:
        psql(f"DROP DATABASE IF EXISTS {DB} WITH (FORCE)", db="postgres", user="postgres")
        psql(f"CREATE DATABASE {DB} OWNER paperless TEMPLATE paperless_snapshot STRATEGY FILE_COPY",
             db="postgres", user="postgres")
        print("scaling to ~100 000 documents ...", flush=True)
        psql(file=SCALE)
        psql("VACUUM ANALYZE")
    ndocs = int(psql("SELECT count(*) FROM documents_document").strip())
    nobj = int(psql("SELECT (SELECT count(*) FROM documents_document) + (SELECT count(*) FROM documents_tag)"
                    " + (SELECT count(*) FROM documents_correspondent) + (SELECT count(*) FROM auth_user)").strip())
    print(f"{ndocs} documents; setup {time.perf_counter() - t0:.0f}s", flush=True)

    print("without triggers ...", flush=True)
    base = measure(a.repeat)

    print("installing pl_sync migration (initial fill) ...", flush=True)
    t1 = time.perf_counter()
    psql(file=MIGRATION, single_tx=True)
    fill_s = time.perf_counter() - t1
    log_rows = int(psql("SELECT count(*) FROM paperrust_sync_log").strip())
    psql("VACUUM ANALYZE")

    print("with triggers ...", flush=True)
    trig = measure(a.repeat)

    # Sequencer: commit-change 10,000 documents, then number them
    timed(OPS["set_correspondent 10 000 Dok. (qs.update)"], commit=True)
    pend = int(psql("SELECT count(*) FROM paperrust_sync_pending").strip())
    seq_ms = timed("SELECT paperrust_sync_sequence(50000)", commit=True)
    t2 = time.perf_counter()
    feed = psql("SELECT count(*), sum(length(uuid::text)) FROM (SELECT * FROM paperrust_sync_log ORDER BY seq) x")
    feed_ms = (time.perf_counter() - t2) * 1000
    rebuild_ms = timed("SELECT paperrust_sync_rebuild()", commit=True)
    sizes = psql("SELECT pg_size_pretty(pg_total_relation_size('paperrust_sync_object')), "
                 "pg_size_pretty(pg_total_relation_size('paperrust_sync_log'))").strip()

    stamp = dt.datetime.now().strftime("%Y-%m-%d_%H%M%S")
    out = REPO / "results" / "sync" / f"load-{stamp}.md"
    out.parent.mkdir(parents=True, exist_ok=True)
    L = [f"# Sync-Änderungserfassung – Lastmessung {stamp}", "",
         f"- DB `{DB}` (paperless_snapshot + tests/pgvalidate/scale_100k.sql): {ndocs} Dokumente, "
         f"~{nobj} erfasste Objekte; PostgreSQL lokal (reference/pg), Laptop, parallel laufende Builds anderer Agenten",
         f"- Messung: `EXPLAIN (ANALYZE)` je Anweisung in einer Transaktion mit ROLLBACK (Median aus {a.repeat} "
         "nach 1 Aufwärmlauf, VACUUM vor jeder Operation). „davon Trigger“ = gemessene Zeit der "
         "paperrust_sync-Trigger in derselben Ausführung; Δ = Trigger-Zeit / übrige Zeit (ohne Streuung "
         "zwischen Läufen). „ohne“ = getrennter Lauf vor der Installation. Werkzeug `tests/sync/load_bench.py`", "",
         "## Erstbefüllung (Migration pl_sync.0001_paperrust_sync)", "",
         f"- Laufzeit inkl. Trigger-Anlage: **{fill_s:.1f} s** für {log_rows} Feed-Einträge (Ziel < 60 s)",
         f"- Größe: paperrust_sync_object / paperrust_sync_log = {sizes.replace('|', ' / ')}",
         f"- `paperrust_sync_rebuild()` erneut (alle Objekte neu nummerieren): {rebuild_ms / 1000:.1f} s", "",
         "## Massenänderungen ohne / mit Triggern", "",
         "| Operation | ohne (ms) | mit (ms) | davon Trigger (ms) | Δ |",
         "|---|---:|---:|---:|---:|"]
    for name in OPS:
        b = statistics.median(x[0] for x in base[name])
        t = statistics.median(x[0] for x in trig[name])
        tr = statistics.median(x[1] for x in trig[name])
        if tr > 0:
            delta = f"{tr / max(t - tr, 0.001) * 100:+.0f} %"
        else:
            delta = f"{(t / b - 1) * 100:+.0f} % (Lauf-Vergleich)"
        L.append(f"| {name} | {b:.0f} | {t:.0f} | {tr:.0f} | {delta} |")
    L += ["", "## Sequenzer und Feed", "",
          f"- {pend} Wartelisten-Zeilen (10 000 Dokumente, eine Anweisung) → `paperrust_sync_sequence()` "
          f"**{seq_ms:.0f} ms** (≈ {pend / max(seq_ms, 1) * 1000:.0f} Zeilen/s)",
          f"- Gesamter Feed ({feed.split('|')[0].strip()} Einträge, nach seq) per psql gelesen: {feed_ms:.0f} ms "
          "(inkl. Prozessstart)", ""]
    out.write_text("\n".join(L) + "\n", encoding="utf-8")
    sys.stdout.reconfigure(errors="replace")
    print("\n".join(L))
    print(f"report: {out}")
    if not a.keep:
        psql(f"DROP DATABASE IF EXISTS {DB} WITH (FORCE)", db="postgres", user="postgres")
    return 0


if __name__ == "__main__":
    sys.exit(main())
