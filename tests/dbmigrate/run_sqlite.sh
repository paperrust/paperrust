#!/usr/bin/env bash
# End-to-end test SQLite → PostgreSQL for paperrust-migrate-db.
#   1. Source: upstream v3.4.2 `migrate` on SQLite (own temp data directory) + reference/seed.py
#      + tests/dbmigrate/seed_edge.py (edge cases, EDGE_BULK bulk documents)
#   2. --dry-run, move, --verify-only, second move onto a filled target (must abort)
#   3. Source byte-identical (SHA-256 before/after)
#   4. NUL byte case: copy of the source with U+0000 → default aborts, --nul replace reports + takes over
# Target DBs: pl_dbmigrate_test_<WORK_NAME>[_nul] (own; `paperless` is never touched).
# Invocation: bash tests/dbmigrate/run_sqlite.sh
#   Bulk test: WORK_NAME=bulk EDGE_BULK=100000 bash tests/dbmigrate/run_sqlite.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
WORK="$HERE/.work/${WORK_NAME:-sqlite}"
BIN="${CARGO_TARGET_DIR:-$REPO_DIR/target}/debug/paperrust-migrate-db.exe"
TDB=pl_dbmigrate_test_${WORK_NAME:-sqlite}
TDB_NUL=pl_dbmigrate_test_${WORK_NAME:-sqlite}_nul
fail() { echo "FEHLER: $*"; exit 1; }

PSQL_SU=("$PG_BIN/psql" -X -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -q)
newdb() { PGPASSWORD="$PG_SUPERPASS" "${PSQL_SU[@]}" -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" \
  -c "CREATE DATABASE $1 OWNER paperless ENCODING 'UTF8' TEMPLATE template0"; }

if [[ "${REUSE_SOURCE:-0}" != 1 ]]; then
  rm -rf "$WORK"; mkdir -p "$WORK"/{data,media,consume,logs}
  (
    unset PAPERLESS_DBHOST PAPERLESS_DBPORT PAPERLESS_DBNAME PAPERLESS_DBUSER PAPERLESS_DBPASS
    export PAPERLESS_DBENGINE=sqlite
    export PAPERLESS_DATA_DIR="$WORK/data" PAPERLESS_MEDIA_ROOT="$WORK/media"
    export PAPERLESS_CONSUMPTION_DIR="$WORK/consume" PAPERLESS_LOGGING_DIR="$WORK/logs"
    export DJANGO_SETTINGS_MODULE=paperless.settings PYTHONPATH="$REPO_DIR/upstream/src"
    echo "[src] migrate (SQLite)"; "$PY" "$REPO_DIR/upstream/src/manage.py" migrate --no-input >"$WORK/logs/migrate.log" 2>&1 \
      || { tail -20 "$WORK/logs/migrate.log"; exit 1; }
    echo "[src] reference/seed.py"; "$PY" -I "$REPO_DIR/reference/seed.py" >"$WORK/logs/seed.log" 2>&1 || { tail -20 "$WORK/logs/seed.log"; exit 1; }
    echo "[src] seed_edge.py (EDGE_BULK=${EDGE_BULK:-0})"; "$PY" -I "$HERE/seed_edge.py"
  )
fi
SRC_FILE="$WORK/data/db.sqlite3"
SRC="sqlite:///$(cygpath -m "$SRC_FILE")"
TGT="postgres://paperless:paperless@127.0.0.1:$PGPORT/$TDB"
ls -la "$WORK/data"
# Database file + WAL must stay byte-identical. db.sqlite3-shm is the WAL index (shared
# memory, no data), which SQLite updates even with read-only connections → excluded.
src_hash() { sha256sum "$WORK/data/db.sqlite3" $(ls "$WORK/data/db.sqlite3-wal" 2>/dev/null) | sed "s#  .*/#  #"; }
before=$(src_hash)

echo "[1] --dry-run"
"$BIN" --dry-run --quiet --source "$SRC" --report "$WORK/dry-run.json" | tail -4 || fail "dry-run"

newdb "$TDB"
echo "[2] Umzug"
SECONDS=0
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w "$HERE/peakmem.ps1")" "$(cygpath -w "$BIN")" \
  "$(cygpath -w "$WORK/migrate.out")" "$(cygpath -w "$WORK/migrate.err")" \
  --source "$SRC" --target "$TGT" --report "$(cygpath -m "$WORK/migrate.json")" </dev/null >"$WORK/peak.txt" \
  || { cat "$WORK/migrate.out"; tail -5 "$WORK/migrate.err"; fail "Umzug"; }
tail -4 "$WORK/migrate.out"; echo "    Dauer ${SECONDS} s, Werkzeug $(tr -d '\r' < "$WORK/peak.txt")"

echo "[3] --verify-only"
"$BIN" --verify-only --quiet --source "$SRC" --target "$TGT" --report "$WORK/verify.json" | tail -2 || fail "verify-only"

echo "[4] zweiter Umzug auf befülltes Ziel muss abbrechen"
if "$BIN" --quiet --source "$SRC" --target "$TGT" >"$WORK/second.out" 2>&1; then fail "zweiter Umzug lief durch"; fi
grep -q "nicht leer" "$WORK/second.out" || fail "Meldung 'nicht leer' fehlt"

after=$(src_hash)
[[ "$before" == "$after" ]] || fail "Quelle verändert!"
echo "[5] Quelle unverändert (SHA-256 von db.sqlite3 und -wal gleich)"

[[ "${SKIP_NUL:-0}" == 1 ]] && { echo "OK: SQLite-Test bestanden (NUL-Fall übersprungen)"; exit 0; }
echo "[6] NUL-Byte"
cp "$SRC_FILE" "$WORK/nul.sqlite3"
"$PY" -I -c "
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
c.execute(\"UPDATE documents_note SET note = 'vor' || char(0) || 'nach' WHERE id = (SELECT min(id) FROM documents_note)\")
c.execute(\"UPDATE documents_tag SET name = 'tag' || char(0) WHERE id = (SELECT min(id) FROM documents_tag)\")
c.commit(); c.close()" "$WORK/nul.sqlite3"
NSRC="sqlite:///$(cygpath -m "$WORK/nul.sqlite3")"
NTGT="postgres://paperless:paperless@127.0.0.1:$PGPORT/$TDB_NUL"
newdb "$TDB_NUL"
if "$BIN" --quiet --source "$NSRC" --target "$NTGT" --report "$WORK/nul-fail.json" >"$WORK/nul-fail.out" 2>&1; then fail "NUL ohne Policy lief durch"; fi
grep -q "NUL" "$WORK/nul-fail.out" || fail "NUL-Meldung fehlt"
n=$("$PG_BIN/psql" -X -At -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$TDB_NUL" -c "select count(*) from pg_tables where schemaname='public'")
[[ "$n" == 0 ]] || fail "Ziel nach Abbruch nicht leer ($n Tabellen)"
echo "    Standard: Abbruch, Ziel leer"
"$BIN" --dry-run --quiet --source "$NSRC" >"$WORK/nul-dry.out" 2>&1 || true
grep -c "FEHLER " "$WORK/nul-dry.out" | xargs echo "    dry-run meldet Zellen:"
"$BIN" --quiet --nul replace --source "$NSRC" --target "$NTGT" --report "$WORK/nul-replace.json" >"$WORK/nul-replace.out" 2>&1 \
  || { cat "$WORK/nul-replace.out"; fail "--nul replace"; }
grep "GEÄNDERT" "$WORK/nul-replace.out"
"$PG_BIN/psql" -X -At -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$TDB_NUL" -c "select note from documents_note order by id limit 1"
echo "OK: SQLite-Test bestanden"
