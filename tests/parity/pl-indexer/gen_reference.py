"""Parity run for pl-indexer: upstream (unchanged) index vs. Rust index from the same DB.

1. Builds the search index with the UNCHANGED upstream code (`manage.py document_index
   reindex`, in-process via call_command) from the reference DB into a temp folder.
   Only overrides via environment, before Django starts: PAPERLESS_DATA_DIR (-> INDEX_DIR
   = <out>/py_data/index) and PAPERLESS_LOGGING_DIR. The DB session is forced read-only
   (SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY on every new connection), so
   nothing in the reference DB can change. reference/data is never touched.
2. Optionally (--run-rust) runs the Rust side: crates/pl-indexer/tests/parity.rs, which
   rebuilds the same index with pl-indexer into <out>/rust_index and compares stored
   fields, terms/postings/positions, field norms, fast fields and, through pl-search,
   searches/scores/highlights, autocomplete and more-like-this. Report: <out>/report.json.
   With --incremental, upstream additionally runs write batches (add_or_update_ids,
   add_or_update of a version, remove) on a copy of its index; the Rust test
   `incremental_parity` runs the same operations with pl-indexer on another copy and
   compares the results (report_incremental.json).

Usage (Git-Bash):
    source reference/env.sh
    "$PY" tests/parity/pl-indexer/gen_reference.py --out "$(mktemp -d)" --incremental --run-rust
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]


def build_upstream_index(out: Path) -> dict:
    data_dir = out / "py_data"
    index_dir = data_dir / "index"
    index_dir.mkdir(parents=True, exist_ok=True)
    (out / "logs").mkdir(parents=True, exist_ok=True)
    os.environ["PAPERLESS_DATA_DIR"] = str(data_dir)
    os.environ["PAPERLESS_LOGGING_DIR"] = str(out / "logs")
    os.environ.setdefault("DJANGO_SETTINGS_MODULE", "reference_settings")

    import django
    from django.db.backends.signals import connection_created

    def _read_only(sender, connection, **kwargs):  # noqa: ARG001
        with connection.cursor() as cur:
            cur.execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")

    connection_created.connect(_read_only)
    django.setup()

    from django.conf import settings
    from django.core.management import call_command

    assert Path(settings.INDEX_DIR).resolve() == index_dir.resolve(), settings.INDEX_DIR
    started = time.monotonic()
    call_command("document_index", "reindex", no_progress_bar=True)
    elapsed = time.monotonic() - started

    import tantivy

    idx = tantivy.Index.open(str(index_dir))
    num_docs = idx.searcher().num_docs
    return {
        "index_dir": str(index_dir),
        "num_docs": num_docs,
        "seconds": round(elapsed, 3),
        "search_language": settings.SEARCH_LANGUAGE,
        "tantivy_py": getattr(tantivy, "__version__", "?"),
    }


# Incremental writes run on a COPY of the fresh upstream index (inc_base) by upstream
# (py_inc) and by Rust (rs_inc, crates/pl-indexer/tests/parity.rs::incremental_parity).
# Keep in sync with incremental_parity() in parity.rs.
def run_upstream_incremental(out: Path) -> dict:
    import shutil

    from django.conf import settings

    from documents.models import Document
    from documents.search import get_backend
    from documents.search import reset_backend

    src = out / "py_data" / "index"
    base = out / "inc_base"
    py_inc = out / "py_inc"
    for d in (base, py_inc):
        if d.exists():
            shutil.rmtree(d)
        shutil.copytree(src, d)
    settings.INDEX_DIR = py_inc
    reset_backend()
    backend = get_backend()
    # 1) bulk path (bulk_update_documents): versions map to roots, trash/unknown skipped
    with backend.batch_update() as batch:
        batch.add_or_update_ids([1, 3, 201, 196, 999999])
        batch.remove(5)
    # 2) single path with a version instance (signals add_to_index / admin)
    with backend.batch_update() as batch:
        batch.add_or_update(Document.global_objects.get(pk=202))
    # 3) convenience methods (10 s lock)
    backend.remove(7)
    backend.add_or_update(Document.objects.get(pk=8))
    reset_backend()
    return {"inc_base": str(base), "py_inc": str(py_inc)}


def run_rust(out: Path) -> int:
    env = dict(os.environ)
    env["PAPERRUST_INDEXER_PARITY_DIR"] = str(out)
    env.setdefault("CARGO_TARGET_DIR", str(REPO / "target"))
    cmd = [
        "cargo", "test", "-p", "pl-indexer", "--test", "parity", "-j", "1",
        "--", "--ignored", "--nocapture", "--test-threads", "1",
    ]
    return subprocess.call(cmd, cwd=str(REPO), env=env)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, help="empty temp folder")
    ap.add_argument("--run-rust", action="store_true")
    ap.add_argument("--incremental", action="store_true", help="also run upstream write batches on a copy")
    args = ap.parse_args()
    out = Path(args.out).resolve()
    ref_data = (REPO / "reference" / "data").resolve()
    if out == ref_data or ref_data in out.parents:
        print("refusing to write below reference/data", file=sys.stderr)
        return 2
    info = build_upstream_index(out)
    if args.incremental:
        info["incremental"] = run_upstream_incremental(out)
    (out / "upstream.json").write_text(json.dumps(info, indent=2))
    print(json.dumps(info, indent=2))
    if args.run_rust:
        return run_rust(out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
