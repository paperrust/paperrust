#!/usr/bin/env bash
# Additional cases (after run_sqlite.sh, and for [3] after the bulk test WORK_NAME=bulk):
#   [1] PostgreSQL → PostgreSQL (source = target of the SQLite test)
#   [2] upstream `manage.py migrate --check` on the target: Django recognizes it as up to date
#   [3] crash in the middle of COPY (taskkill /F) → target stays empty
#   [4] wrong migration state (row missing / newer migration) → abort with hint
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
BIN="${CARGO_TARGET_DIR:-$REPO_DIR/target}/debug/paperrust-migrate-db.exe"
W="$HERE/.work/extra"; mkdir -p "$W"
fail() { echo "FEHLER: $*"; exit 1; }
PG="postgres://paperless:paperless@127.0.0.1:$PGPORT"
newdb() { PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -X -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres \
  -c "DROP DATABASE IF EXISTS $1 WITH (FORCE)" -c "CREATE DATABASE $1 OWNER paperless ENCODING 'UTF8' TEMPLATE template0"; }
ntables() { "$PG_BIN/psql" -X -At -h 127.0.0.1 -p "$PGPORT" -U paperless -d "$1" -c "select count(*) from pg_class c join pg_namespace n on n.oid=c.relnamespace where n.nspname='public'"; }

echo "[1] PostgreSQL → PostgreSQL"
newdb pl_dbmigrate_test_pg2pg
"$BIN" --quiet --source "$PG/pl_dbmigrate_test_sqlite" --target "$PG/pl_dbmigrate_test_pg2pg" --report "$W/pg2pg.json" | tail -2 || fail "pg2pg"
"$BIN" --verify-only --quiet --source "$PG/pl_dbmigrate_test_sqlite" --target "$PG/pl_dbmigrate_test_pg2pg" | tail -1 || fail "pg2pg verify"

echo "[2] Django migrate --check auf dem Ziel"
( export PAPERLESS_DBNAME=pl_dbmigrate_test_sqlite DJANGO_SETTINGS_MODULE=paperless.settings PYTHONPATH="$REPO_DIR/upstream/src"
  export PAPERLESS_DATA_DIR="$W/data" PAPERLESS_MEDIA_ROOT="$W/media" PAPERLESS_LOGGING_DIR="$W/logs"; mkdir -p "$W"/{data,media,logs}
  "$PY" "$REPO_DIR/upstream/src/manage.py" migrate --check --no-input >"$W/migrate-check.log" 2>&1 ) \
  && echo "    keine offenen Migrationen" || { tail "$W/migrate-check.log"; fail "Django sieht offene Migrationen"; }

BULK="$HERE/.work/bulk/data/db.sqlite3"
if [[ -f "$BULK" ]]; then
  echo "[3] Absturz während des Umzugs"
  newdb pl_dbmigrate_test_crash
  "$BIN" --quiet --source "sqlite:///$(cygpath -m "$BULK")" --target "$PG/pl_dbmigrate_test_crash" >"$W/crash.out" 2>&1 &
  bpid=$!; sleep 25
  wpid=$(cat /proc/$bpid/winpid 2>/dev/null || true)
  n_during=$("$PG_BIN/psql" -X -At -h 127.0.0.1 -p "$PGPORT" -U paperless -d postgres -c "select count(*) from pg_stat_activity where datname='pl_dbmigrate_test_crash' and state <> 'idle'")
  [[ -n "$wpid" ]] && taskkill //F //PID "$wpid" >/dev/null 2>&1 || kill -9 $bpid
  wait $bpid 2>/dev/null || true
  sleep 3
  n=$(ntables pl_dbmigrate_test_crash)
  echo "    aktive Sitzungen vor dem Abbruch: $n_during, Relationen im Ziel danach: $n"
  [[ "$n" == 0 ]] || fail "Ziel nach Absturz nicht leer"
fi

echo "[4] falscher Migrationsstand"
SMALL="$HERE/.work/sqlite/data/db.sqlite3"
cp "$SMALL" "$W/old.sqlite3"; cp "$SMALL" "$W/new.sqlite3"
"$PY" -I -c "
import sqlite3, sys
c = sqlite3.connect(sys.argv[1]); c.execute(\"DELETE FROM django_migrations WHERE app='documents' AND name='0027_documentbarcode'\"); c.commit()
c = sqlite3.connect(sys.argv[2]); c.execute(\"INSERT INTO django_migrations(app, name, applied) VALUES ('documents', '0028_future', '2027-01-01 00:00:00')\"); c.commit()
" "$W/old.sqlite3" "$W/new.sqlite3"
for f in old new; do
  if "$BIN" --dry-run --quiet --source "sqlite:///$(cygpath -m "$W/$f.sqlite3")" >"$W/$f.out" 2>&1; then fail "$f: lief durch"; fi
  grep -q "erst paperless-ngx auf 3.4.2" "$W/$f.out" || grep -q "zuerst paperless-ngx auf 3.4.2" "$W/$f.out" || fail "$f: Hinweis fehlt"
  grep -E "fehlend|unbekannt" "$W/$f.out" | sed 's/^/    /'
done
echo "OK: Zusatzfälle bestanden"
