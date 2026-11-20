#!/usr/bin/env bash
# Local PaperRust test server for the Light UI image comparison (package F0), WITHOUT Django.
#   DB      pl_light_f0 (fresh from TEMPLATE paperless_snapshot), 127.0.0.1:54329; `start` runs
#           `manage migrate` beforehand (idempotent, without pg_dump backup: test copy)
#   Data    frontend-light/.e2e/{data,logs}  (from reference/snapshot/data.tar)
#   Server  127.0.0.1:8330, UI frontend-light/dist under /rs/app/ (/ redirects
#           there), PAPERLESS_STATICDIR=results/static (scripts/build-static.sh, generated on demand)
#   server.sh [start|stop|reset|drop]   (drop = stop server + delete DB)
# Environment: F0_BIN (default results/bin/paperrust-f1.exe), F0_ROUTES (routes-w23.toml), F0_PORT, F0_DB, F0_WORK,
#           F0_ARGS (default "serve"; "serve --worker" also starts processing), F0_LIGHT_DIR (default
#           frontend-light/dist), F0_STATIC (default results/static)
# Local test values only, never against the DB "paperless" or production servers.
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$HERE/../../reference/env.sh"
W="${F0_WORK:-$REPO_DIR/frontend-light/.e2e}"
PORT="${F0_PORT:-8330}"
DB="${F0_DB:-pl_light_f0}"
BIN="${F0_BIN:-$REPO_DIR/results/bin/paperrust-f1.exe}"
PIDF="$W/run/rust.pid"
mkdir -p "$W/run" "$W/logs"

export PAPERLESS_DBNAME=$DB
export PAPERLESS_RS_BIND="127.0.0.1:$PORT"
export PAPERLESS_URL="http://127.0.0.1:$PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$PORT,http://localhost:$PORT,http://127.0.0.1:4173,http://localhost:4173,http://127.0.0.1:4174,http://localhost:4174${F0_PREVIEW_PORT:+,http://127.0.0.1:$F0_PREVIEW_PORT}"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
R="${F0_ROUTES:-$REPO_DIR/results/parity/w23/routes-w23.toml}"
export PAPERLESS_RS_ROUTES="$(cygpath -w "$R")"
unset PAPERLESS_CACHE_BACKEND
export PAPERLESS_DATA_DIR="$(cygpath -w "$W/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W/data/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$W/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W/logs")"
STATIC="${F0_STATIC:-$REPO_DIR/results/static}"
export PAPERLESS_STATICDIR="$(cygpath -w "$STATIC")"
export PAPERRUST_LIGHT_UI_DIR="$(cygpath -w "${F0_LIGHT_DIR:-$REPO_DIR/frontend-light/dist}")"
export PAPERRUST_BRANDING="${F0_BRANDING:-off}"  # F0_BRANDING=on: like in real operation
export PAPERRUST_UPDATE_CHECK=false
export PAPERLESS_RS_LOG="info"
export NO_COLOR=1

alive() { [[ -f "$PIDF" ]] && tasklist //FI "PID eq $(cat "$PIDF")" 2>/dev/null | grep -qi paperrust; }
psql_su() { env PGPASSWORD="$PG_SUPERPASS" "$PG_BIN/psql" -h 127.0.0.1 -p "$PGPORT" -U "$PG_SUPERUSER" -d postgres -v ON_ERROR_STOP=1 -q "$@"; }

reset() {
  psql_su -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" -c "CREATE DATABASE $DB TEMPLATE paperless_snapshot OWNER paperless" || return 1
  rm -rf "$W/data"; mkdir -p "$W/data/consume" "$W/data/scratch"
  tar xf "$REPO_DIR/reference/snapshot/data.tar" -C "$W/data"
  echo "[f0] $DB + $W/data zurückgesetzt"
}

start() {
  if alive; then echo "[f0] läuft bereits (PID $(cat "$PIDF"))"; return 0; fi
  [[ -f "$STATIC/base.css" ]] || bash "$REPO_DIR/scripts/build-static.sh" "$STATIC" || return 1
  [[ -f "$(cygpath -u "$PAPERRUST_LIGHT_UI_DIR")/index.html" ]] || { echo "[f0] Oberfläche fehlt (node node_modules/vite/bin/vite.js build in frontend-light)"; return 1; }
  # Like Docker/Desktop: migrate before every start (idempotent). Otherwise the server no longer starts
  # on a DB without PaperRust migrations (test log 2026-11-16 A-181/B-312). Test copy →
  # without pg_dump backup before the data model restructuring steps.
  PAPERRUST_DATAMODEL_BACKUP=off "$BIN" manage migrate --no-input >"$W/logs/migrate.log" 2>&1 \
    || { echo "[f0] manage migrate fehlgeschlagen (siehe $W/logs/migrate.log)"; tail -5 "$W/logs/migrate.log"; return 1; }
  detach "$PIDF" "$BIN" "$W/logs/rust.out.log" "$W/logs/rust.err.log" ${F0_ARGS:-serve}
  for _ in $(seq 1 90); do
    c=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/" || true)
    [[ "$c" =~ ^(200|302|401|403)$ ]] && { echo "[f0] PaperRust :$PORT PID $(cat "$PIDF")"; return 0; }
    sleep 1
  done
  echo "[f0] Start fehlgeschlagen"; return 1
}

stop() {
  if alive; then taskkill //PID "$(cat "$PIDF")" //T //F >/dev/null 2>&1 && echo "[f0] gestoppt (PID $(cat "$PIDF"))"; fi
  rm -f "$PIDF"
}

case "${1:-start}" in
  start) start ;;
  stop) stop ;;
  reset) reset ;;
  drop) stop; psql_su -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)" && echo "[f0] $DB gelöscht" ;;
  *) echo "usage: server.sh [start|stop|reset|drop]"; exit 2 ;;
esac
