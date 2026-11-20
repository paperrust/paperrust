#!/usr/bin/env bash
# PaperRust test server for the outage tests (tests/chaos), against the OWN throwaway PostgreSQL
# (tests/chaos/pg.sh, port 54400) – never against the shared reference instance 54329.
#   Port 58460, DB pl_chaos_a, working folder %TEMP%/paperrust-chaos/<DB>, "serve --worker".
#   The binary is copied to $CH_WORK/bin (a running .exe locks target/debug/paperrust.exe).
# Invocation: tests/chaos/server.sh [start|stop|kill|restart|reset|reset-empty|drop|env|manage …]
#   reset       = DB from reference/snapshot/paperless.dump + media from data.tar
#   reset-empty = empty DB + empty folders (fresh system for recovery)
#   manage …    = "paperrust manage …" with the same environment (e.g. document_sanity_checker)
# Environment: CH_PORT, CH_DB (must start with pl_chaos_), CH_WORK, CH_BIN, CH_PGPORT, CH_WORKERS,
#           CH_MEDIA (other media folder, e.g. small drive), CH_EXTRA_ENV ("K=V K2=V2").
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
source "$REPO/reference/env.sh" >/dev/null 2>&1
TMPW="$(cygpath -u "${TEMP:-/tmp}")"
CH_ROOT="${CH_ROOT:-$TMPW/paperrust-chaos}"
CH_PORT="${CH_PORT:-58460}"
CH_DB="${CH_DB:-pl_chaos_a}"
CH_PGPORT="${CH_PGPORT:-54400}"
[[ "$CH_PGPORT" == "54329" ]] && { echo "[chaos] 54329 ist die geteilte Referenz-DB - tabu"; exit 2; }
case "$CH_DB" in pl_chaos_*) ;; *) echo "[chaos] DB-Name muss mit pl_chaos_ beginnen"; exit 2 ;; esac
W="${CH_WORK:-$CH_ROOT/$CH_DB}"
SRC_BIN="${CH_BIN:-$REPO/target/debug/paperrust.exe}"
BIN="$CH_ROOT/bin/paperrust.exe"
PIDF="$W/run/rust.pid"
mkdir -p "$W/run" "$W/logs"

export PAPERLESS_DBHOST=127.0.0.1 PAPERLESS_DBPORT="$CH_PGPORT" PAPERLESS_DBNAME="$CH_DB"
export PAPERLESS_DBUSER=paperless PAPERLESS_DBPASS=paperless
export PGPORT="$CH_PGPORT"
export PAPERRUST_BIND="127.0.0.1:$CH_PORT"
export PAPERLESS_URL="http://127.0.0.1:$CH_PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$CH_PORT,http://localhost:$CH_PORT"
export PAPERLESS_ALLOWED_HOSTS="127.0.0.1,localhost"
export PAPERRUST_ROUTES="$(cygpath -w "$REPO/routes.toml")"
unset PAPERLESS_CACHE_BACKEND PAPERLESS_REDIS
export PAPERLESS_DATA_DIR="$(cygpath -w "$W/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "${CH_MEDIA:-$W/data/media}")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W/data/consume")"
export PAPERLESS_SCRATCH_DIR="$(cygpath -w "$W/data/scratch")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W/logs")"
export PAPERLESS_STATICDIR="$(cygpath -w "$REPO/results/static")"
[[ -f "$REPO/frontend-light/dist/index.html" ]] && export PAPERRUST_LIGHT_UI_DIR="$(cygpath -w "$REPO/frontend-light/dist")"
export PAPERRUST_BRANDING=off PAPERRUST_UPDATE_CHECK=false PAPERRUST_LOG="${PAPERRUST_LOG:-info}" NO_COLOR=1
export PAPERLESS_TASK_WORKERS="${CH_WORKERS:-2}"
export PAPERRUST_DATAMODEL_BACKUP=off
TOOLS="$(cygpath -u "${LOCALAPPDATA:-C:/Users/$USERNAME/AppData/Local}")/Programs/PaperRust"
[[ -z "${PAPERRUST_PDFIUM_PATH:-}" && -f "$TOOLS/pdfium/pdfium.dll" ]] && export PAPERRUST_PDFIUM_PATH="$(cygpath -w "$TOOLS/pdfium/pdfium.dll")"
if [[ -z "${PAPERRUST_TESSERACT_PATH:-}" && -f "$TOOLS/tesseract/tesseract.exe" ]]; then
  export PAPERRUST_TESSERACT_PATH="$(cygpath -w "$TOOLS/tesseract/tesseract.exe")"
  export PAPERRUST_TESSDATA_DIR="$(cygpath -w "$TOOLS/tesseract/tessdata")"
fi
for kv in ${CH_EXTRA_ENV:-}; do export "$kv"; done

alive() { [[ -f "$PIDF" ]] && tasklist //FI "PID eq $(cat "$PIDF")" 2>/dev/null | grep -qi paperrust; }
copy_bin() { mkdir -p "$CH_ROOT/bin"; if [[ ! -f "$BIN" || "$SRC_BIN" -nt "$BIN" ]]; then cp -f "$SRC_BIN" "$BIN" || exit 1; fi; }
pg() { CH_PGPORT="$CH_PGPORT" CH_WORK="$CH_ROOT" bash "$HERE/pg.sh" "$@"; }

reset() {
  stop
  pg createdb "$CH_DB" --snapshot || return 1
  rm -rf "$W/data"; mkdir -p "$W/data/consume" "$W/data/scratch"
  tar xf "$REPO/reference/snapshot/data.tar" -C "$W/data"
  echo "[chaos] $CH_DB + $W/data zurückgesetzt (Snapshot)"
}
reset_empty() {
  stop
  pg createdb "$CH_DB" || return 1
  rm -rf "$W/data"; mkdir -p "$W/data/consume" "$W/data/scratch" "$W/data/data" "$W/data/media"
  echo "[chaos] $CH_DB + $W/data leer"
}

start() {
  if alive; then echo "[chaos] läuft bereits (PID $(cat "$PIDF"))"; return 0; fi
  copy_bin
  [[ -d "$W/data" ]] || reset || return 1
  if [[ "${CH_SKIP_MIGRATE:-}" != 1 ]]; then
    "$BIN" manage migrate --no-input >"$W/logs/migrate.log" 2>&1 \
      || { echo "[chaos] manage migrate fehlgeschlagen ($W/logs/migrate.log)"; tail -5 "$W/logs/migrate.log"; return 1; }
  fi
  detach "$PIDF" "$BIN" "$W/logs/rust.out.log" "$W/logs/rust.err.log" ${CH_ARGS:-serve --worker}
  for _ in $(seq 1 120); do
    c=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$CH_PORT/api/" || true)
    [[ "$c" =~ ^(200|302|401|403)$ ]] && { echo "[chaos] PaperRust :$CH_PORT PID $(cat "$PIDF")"; return 0; }
    sleep 1
  done
  echo "[chaos] Start fehlgeschlagen"; tail -20 "$W/logs/rust.err.log"; return 1
}
stop() {
  if alive; then taskkill //PID "$(cat "$PIDF")" //T //F >/dev/null 2>&1 && echo "[chaos] gestoppt (PID $(cat "$PIDF"))"; fi
  rm -f "$PIDF"
}

case "${1:-start}" in
  start) start ;;
  stop|kill) stop ;;
  restart) stop; start ;;
  reset) reset ;;
  reset-empty) reset_empty ;;
  drop) stop; pg dropdb "$CH_DB"; rm -rf "$W"; echo "[chaos] $CH_DB + $W gelöscht" ;;
  manage)
    # own copy: the server copy is locked as long as the server runs
    shift; MB="$CH_ROOT/bin/paperrust-manage.exe"; mkdir -p "$CH_ROOT/bin"
    if [[ ! -f "$MB" || "$SRC_BIN" -nt "$MB" ]]; then cp -f "$SRC_BIN" "$MB" || exit 1; fi
    "$MB" manage "$@" ;;
  env)
    echo "CH_URL=http://127.0.0.1:$CH_PORT"
    echo "CH_PID=$(cat "$PIDF" 2>/dev/null)"
    echo "CH_WORK=$(cygpath -w "$W")"
    echo "CH_MEDIA=$PAPERLESS_MEDIA_ROOT"
    echo "CH_DATA=$PAPERLESS_DATA_DIR"
    echo "CH_CONSUME=$PAPERLESS_CONSUMPTION_DIR"
    echo "CH_LOG=$(cygpath -w "$W/logs")"
    echo "CH_PSQL=$(cygpath -w "$PG_BIN/psql.exe")"
    echo "CH_DSN=host=127.0.0.1 port=$CH_PGPORT user=paperless password=paperless dbname=$CH_DB"
    ;;
  *) echo "usage: server.sh [start|stop|kill|restart|reset|reset-empty|drop|env|manage …]"; exit 2 ;;
esac
