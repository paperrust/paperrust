"""W2 reset in one process (Windows process start is the bottleneck; see w2-reset.sh).

Restores ONLY the W2 copy:
  * DB pl_w2_ref := TEMPLATE pl_w2_snapshot (created once from paperless_snapshot, read-only use)
  * tests/parity/.w2/data/{data,media} := reference/snapshot/data.tar, consume/ emptied
  * Redis keys with prefix "w2" (PAPERLESS_REDIS_PREFIX of the W2 instances)
Never touches the shared reference (DB paperless, reference/data, other Redis keys).

Instance and capture switch from the environment (w2-env.sh): W2_NAME (default w2), W2_DB,
W2_TPL, W2_RS_PORT; PARITY_SYNC_CAPTURE=on restores from <W2_TPL>_sync instead, a private
template with the pl-sync migration (crates/pl-sync/migrations/0001_paperrust_sync.sql) applied.
It is rebuilt when the migration file changes (sha256 in the database comment).

Stdlib only (psql for the DB). Files are only re-extracted when the tree differs from the
manifest written after the last extraction (name, size, mtime) - most write routes never touch
media, and rmtree+extract costs seconds on this machine.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import tarfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
REF = REPO / "reference"
NAME = os.environ.get("W2_NAME") or "w2"
W2 = REPO / "tests" / "parity" / f".{NAME}"
DB = os.environ.get("W2_DB") or f"pl_{NAME}_ref"
TPL = os.environ.get("W2_TPL") or f"pl_{NAME}_snapshot"
SNAP = "paperless_snapshot"
assert DB not in ("paperless", SNAP) and TPL not in ("paperless", SNAP) and DB != TPL
RS_PORT = int(os.environ.get("W2_RS_PORT") or 8101)
SYNC_CAPTURE = (os.environ.get("PARITY_SYNC_CAPTURE") or "off").lower() in ("1", "on", "true", "yes")
SYNC_MIGRATION = REPO / "crates" / "pl-sync" / "migrations" / "0001_paperrust_sync.sql"
PSQL = REF / "pg" / "pgsql" / "bin" / "psql.exe"
MANIFEST = W2 / "data.manifest.json"
REDIS_PORT = 63799
PREFIX = NAME.encode()


def psql(*sql: str, db: str = "postgres", user: str = "postgres", file: Path | None = None) -> str:
    args = [str(PSQL), "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1", "-p", "54329",
            "-U", user, "-d", db]
    if file is not None:
        args += ["-1", "-f", str(file)]
    for q in sql:
        args += ["-c", q]
    env = dict(os.environ, PGPASSWORD="postgres-local-only" if user == "postgres" else "paperless")
    r = subprocess.run(args, capture_output=True, env=env, timeout=300)
    if r.returncode != 0:
        raise RuntimeError(r.stderr.decode("utf-8", "replace").strip()[-400:])
    return r.stdout.decode("utf-8", "replace")


def sync_template() -> str:
    """<TPL>_sync: TPL + pl-sync migration (PARITY_SYNC_CAPTURE=on), rebuilt on migration change."""
    stpl = f"{TPL}_sync"
    want = "pl_sync:" + hashlib.sha256(SYNC_MIGRATION.read_bytes()).hexdigest()[:16]
    have = psql(f"SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = '{stpl}'").strip()
    if have == want:
        return stpl
    if not psql(f"SELECT 1 FROM pg_database WHERE datname = '{TPL}'").strip():
        psql(f"CREATE DATABASE {TPL} OWNER paperless TEMPLATE {SNAP} STRATEGY FILE_COPY",
             f"ALTER DATABASE {TPL} WITH ALLOW_CONNECTIONS false")
    psql(f"DROP DATABASE IF EXISTS {stpl} WITH (FORCE)",
         f"CREATE DATABASE {stpl} OWNER paperless TEMPLATE {TPL} STRATEGY FILE_COPY")
    psql(db=stpl, user="paperless", file=SYNC_MIGRATION)
    psql(f"COMMENT ON DATABASE {stpl} IS '{want}'", f"ALTER DATABASE {stpl} WITH ALLOW_CONNECTIONS false")
    return stpl


def reset_db() -> None:
    tpl = sync_template() if SYNC_CAPTURE else TPL
    for attempt in range(5):
        try:
            psql(f"DROP DATABASE IF EXISTS {DB} WITH (FORCE)",
                 f"CREATE DATABASE {DB} OWNER paperless TEMPLATE {tpl} STRATEGY WAL_LOG")
            return
        except RuntimeError as e:
            if f'"{TPL}" does not exist' in str(e):
                # once: private template from the seed snapshot (only read)
                psql(f"CREATE DATABASE {TPL} OWNER paperless TEMPLATE {SNAP} STRATEGY FILE_COPY",
                     f"ALTER DATABASE {TPL} WITH ALLOW_CONNECTIONS false")
                continue
            if attempt == 4 or "being accessed" not in str(e):
                raise
            time.sleep(0.5)


def _tree(root: Path) -> list:
    out = []
    for sub in ("data", "media", "consume"):
        base = root / sub
        for dirpath, dirnames, filenames in os.walk(base):
            dirnames.sort()
            rel = os.path.relpath(dirpath, root)
            out.append([rel, -1, 0])
            for fn in sorted(filenames):
                st = os.stat(os.path.join(dirpath, fn))
                out.append([os.path.join(rel, fn), st.st_size, st.st_mtime_ns])
    return out


def reset_files() -> bool:
    data = W2 / "data"
    try:
        if json.loads(MANIFEST.read_text(encoding="utf-8")) == _tree(data):
            return False
    except (OSError, ValueError):
        pass
    for sub in ("data", "media", "consume"):
        shutil.rmtree(data / sub, ignore_errors=True)
    (data / "consume").mkdir(parents=True, exist_ok=True)
    with tarfile.open(REF / "snapshot" / "data.tar") as t:
        t.extractall(data, filter="tar")
    MANIFEST.write_text(json.dumps(_tree(data)), encoding="utf-8")
    return True


def _resp(f):
    line = f.readline().rstrip(b"\r\n")
    t, rest = line[:1], line[1:]
    if t in (b"+", b":"):
        return rest
    if t == b"-":
        raise RuntimeError(rest.decode())
    if t == b"$":
        n = int(rest)
        return None if n < 0 else f.read(n + 2)[:-2]
    if t == b"*":
        return [_resp(f) for _ in range(int(rest))]
    raise RuntimeError(f"bad RESP {line!r}")


def reset_redis() -> None:
    try:
        s = socket.create_connection(("127.0.0.1", REDIS_PORT), 3)
    except OSError:
        return
    with s:
        f = s.makefile("rb")

        def cmd(*a: bytes):
            s.sendall(b"*%d\r\n" % len(a) + b"".join(b"$%d\r\n%s\r\n" % (len(x), x) for x in a))
            return _resp(f)

        cur = b"0"
        while True:
            cur, keys = cmd(b"SCAN", cur, b"MATCH", PREFIX + b"*", b"COUNT", b"1000")
            keys = [k for k in keys or [] if k.startswith(PREFIX)]
            if keys:
                cmd(b"DEL", *keys)
            if cur == b"0":
                break


def warm_up_b() -> int:
    """Drain stale pooled connections of the W2 PaperRust (:8101) after DROP DATABASE ... FORCE.

    pl-core pings a pooled connection only when it was idle >= 30 s, so the first request per
    pooled connection after a reset fails with 500 ("internal auth error"). That is an
    environment artefact of the reset, not a parity finding (reported as CORE-REQUEST).
    Sequential requests until three consecutive 200s; returns the number of failed requests.
    """
    import urllib.error
    import urllib.request

    url = f"http://127.0.0.1:{RS_PORT}/api/ui_settings/"
    hdr = {"Authorization": "Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001", "Accept": "application/json; version=10"}
    ok = failed = 0
    for _ in range(40):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=hdr), timeout=20) as r:
                status = r.status
        except urllib.error.HTTPError as e:
            status = e.code
        except OSError:
            return -1  # B not running
        if status == 200:
            ok += 1
            if ok >= 3:
                break
        else:
            ok = 0
            failed += 1
    return failed


def main() -> int:
    t0 = time.perf_counter()
    reset_db()
    files = reset_files()
    reset_redis()
    stale = warm_up_b()
    print(f"[w2-reset] {DB}{' (sync capture on)' if SYNC_CAPTURE else ''}, "
          f"files {'restored' if files else 'unchanged'}, redis {PREFIX.decode()}*, "
          f"B stale connections {stale} in {time.perf_counter() - t0:.1f}s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
