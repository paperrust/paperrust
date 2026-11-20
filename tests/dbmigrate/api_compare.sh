#!/usr/bin/env bash
# Final test of the DB move: start upstream paperless-ngx v3.4.2 (unchanged, granian as in the Docker image)
# one after the other on the SOURCE (SQLite or MariaDB, port 8101) and on the TARGET (PostgreSQL, port 8102),
# fetch the same selection of GET responses, compare, end the server by PID.
# Same DATA_DIR/MEDIA_ROOT for both (search index is built once from the source and shared by the target
# → verifies that the index is independent of the DB engine).
# Invocation: bash tests/dbmigrate/api_compare.sh [sqlite|mariadb]  (after run_sqlite.sh or run_mariadb.sh)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
KIND="${1:-sqlite}"
WORK="$HERE/.work/$KIND"
TDB="pl_dbmigrate_test_$KIND"
[[ -f "$WORK/migrate.json" ]] || { echo "erst run_$KIND.sh ausführen"; exit 1; }

export PAPERLESS_DATA_DIR="$WORK/data" PAPERLESS_MEDIA_ROOT="$WORK/media"
export PAPERLESS_CONSUMPTION_DIR="$WORK/consume" PAPERLESS_LOGGING_DIR="$WORK/logs"
export PAPERLESS_STATICDIR="$WORK/static"
export DJANGO_SETTINGS_MODULE=paperless.settings PYTHONPATH="$REPO_DIR/upstream/src"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost" PAPERLESS_URL="http://127.0.0.1:8101"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:8101,http://127.0.0.1:8102"
mkdir -p "$WORK/static" "$WORK/run" "$WORK/data/index"

source_env() {
  if [[ "$KIND" == sqlite ]]; then
    unset PAPERLESS_DBHOST PAPERLESS_DBPORT PAPERLESS_DBNAME PAPERLESS_DBUSER PAPERLESS_DBPASS
    export PAPERLESS_DBENGINE=sqlite
  else
    source "$HERE/mariadb_env.sh"
  fi
}
target_env() {
  export PAPERLESS_DBENGINE=postgresql PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT="$PGPORT"
  export PAPERLESS_DBNAME="$TDB" PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless
}

start() { # port name
  local port=$1 name=$2 code=000
  export GRANIAN_HOST=127.0.0.1 GRANIAN_PORT=$port GRANIAN_WORKERS=1
  detach "$WORK/run/$name.pid" "$REF_DIR/venv/Scripts/granian.exe" "$WORK/logs/$name.out.log" "$WORK/logs/$name.err.log" \
    --interface asginl --loop asyncio --working-dir "$(cygpath -w "$REPO_DIR/upstream/src")" "paperless.asgi:application"
  for _ in $(seq 1 120); do
    code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/api/" || true)
    [[ "$code" =~ ^(200|302|401|403)$ ]] && break
    sleep 1
  done
  [[ "$code" =~ ^(200|302|401|403)$ ]] || { tail -20 "$WORK/logs/$name.err.log"; stop "$name"; exit 1; }
  echo "  $name läuft (PID $(cat "$WORK/run/$name.pid"), Port $port)"
}
stop() { # name
  local pid; pid=$(cat "$WORK/run/$1.pid" 2>/dev/null || true)
  [[ -n "$pid" ]] && taskkill //F //T //PID "$pid" >/dev/null 2>&1 || true
  rm -f "$WORK/run/$1.pid"
}
trap 'stop source; stop target' EXIT

(source_env; echo "[api] Suchindex aus der Quelle bauen"
 "$PY" "$REPO_DIR/upstream/src/manage.py" document_index reindex --no-progress-bar >"$WORK/logs/index.log" 2>&1 || tail -5 "$WORK/logs/index.log")

echo "[api] Quelle ($KIND) auf :8101"
(source_env; start 8101 source; "$PY" -I "$HERE/api_fetch.py" http://127.0.0.1:8101 "$WORK/api-source.json"; stop source)
echo "[api] Ziel (PostgreSQL $TDB) auf :8102"
(target_env; start 8102 target; "$PY" -I "$HERE/api_fetch.py" http://127.0.0.1:8102 "$WORK/api-target.json"; stop target)
"$PY" -I "$HERE/api_compare.py" "$WORK/api-source.json" "$WORK/api-target.json" "$WORK/api-compare.json"
