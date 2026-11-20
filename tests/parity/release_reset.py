"""Reset for the write parity run with two databases (tests/parity/release-run.sh).

Since 2026-11-14 PaperRust (B) runs standalone and restructures its database (data-model stages,
tag adjacency list), so A (Django) and B can no longer share one database. Each reset restores:

  A  Django copy W2_NAME (default "w2rel"): DB pl_<name>_ref, data dir tests/parity/.<name>/data,
     Redis prefix <name> - via the functions of tests/parity/w2_reset.py (unchanged).
  B  PaperRust copy RELRUN_B_DB from the migrated template RELRUN_B_TPL, files under RELRUN_B_DIR
     (re-extracted from reference/snapshot/data.tar only when the tree changed).

Afterwards it waits until B answers three times 200 in a row: after DROP DATABASE ... WITH (FORCE)
B's database gate answers 503 for a while ("database not reachable"); that is an artefact of the
reset, not a parity finding.

Never touches paperless, paperless_snapshot or pl_scale_1m. Stdlib only.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import sys
import tarfile
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
os.environ.setdefault("W2_NAME", "w2rel")
os.environ["W2_RS_PORT"] = "1"  # w2_reset's own warm-up is not used (no B on the W2 copy)
_spec = importlib.util.spec_from_file_location("w2_reset", REPO / "tests" / "parity" / "w2_reset.py")
w2 = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(w2)

B_DB = os.environ.get("RELRUN_B_DB", "pl_relrun_bw")
B_TPL = os.environ.get("RELRUN_B_TPL", "pl_relrun_bw_tpl")
B_PORT = int(os.environ.get("RELRUN_B_PORT", "58441"))
B_DIR = Path(os.environ["RELRUN_B_DIR"])
B_MANIFEST = Path(str(B_DIR) + ".manifest.json")
PROTECTED = {"paperless", "paperless_snapshot", "pl_scale_1m"}
assert B_DB not in PROTECTED and B_TPL not in PROTECTED and B_DB != B_TPL
ADMIN_TOKEN = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"  # local test value (reference/users.json)


def _tree() -> list:
    out = []
    for sub in ("data", "media", "consume"):
        for dirpath, dirnames, filenames in os.walk(B_DIR / sub):
            dirnames.sort()
            rel = os.path.relpath(dirpath, B_DIR)
            out.append([rel, -1, 0])
            for fn in sorted(filenames):
                st = os.stat(os.path.join(dirpath, fn))
                out.append([os.path.join(rel, fn), st.st_size, st.st_mtime_ns])
    return out


def reset_b_files() -> bool:
    try:
        if json.loads(B_MANIFEST.read_text(encoding="utf-8")) == _tree():
            return False
    except (OSError, ValueError):
        pass
    for sub in ("data", "media", "consume"):
        shutil.rmtree(B_DIR / sub, ignore_errors=True)
    (B_DIR / "consume").mkdir(parents=True, exist_ok=True)
    with tarfile.open(REPO / "reference" / "snapshot" / "data.tar") as t:
        t.extractall(B_DIR, filter="tar")
    B_MANIFEST.write_text(json.dumps(_tree()), encoding="utf-8")
    return True


def reset_b_db() -> None:
    for attempt in range(5):
        try:
            w2.psql(f"DROP DATABASE IF EXISTS {B_DB} WITH (FORCE)",
                    f"CREATE DATABASE {B_DB} OWNER paperless TEMPLATE {B_TPL} STRATEGY WAL_LOG")
            return
        except RuntimeError as e:
            if attempt == 4 or "being accessed" not in str(e):
                raise
            time.sleep(0.5)


def wait_b(timeout_s: float = 60.0) -> int:
    """Waits for three consecutive 200s from B; returns the number of non-200 answers (-1 = timeout)."""
    hdr = {"Authorization": f"Token {ADMIN_TOKEN}", "Accept": "application/json; version=10"}
    url = f"http://127.0.0.1:{B_PORT}/api/ui_settings/"
    ok = bad = 0
    deadline = time.time() + timeout_s
    while time.time() < deadline and ok < 3:
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=hdr), timeout=20) as r:
                status = r.status
        except urllib.error.HTTPError as e:
            status = e.code
        except OSError:
            status = -1
        if status == 200:
            ok += 1
        else:
            ok = 0
            bad += 1
            time.sleep(0.2)
    return bad if ok >= 3 else -1


def main() -> int:
    t0 = time.perf_counter()
    w2.reset_db()
    fa = w2.reset_files()
    w2.reset_redis()
    reset_b_db()
    fb = reset_b_files()
    bad = wait_b() if B_PORT else 0  # RELRUN_B_PORT=0: B not started yet
    print(f"[release-reset] A {w2.DB} files {'restored' if fa else 'unchanged'}; "
          f"B {B_DB} files {'restored' if fb else 'unchanged'}, 503 answers {bad}; "
          f"{time.perf_counter() - t0:.1f}s")
    if bad < 0:
        print("[release-reset] B did not answer 200 within 60 s", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
